/*!
PDF content operators.

This module provides facilities to read and interpret PDF content streams using
high-level types.

```
use hayro_syntax::object::Number;
use hayro_syntax::content::*;
use hayro_syntax::content::ops::*;

let content_stream = b"1 0 0 -1 0 200 cm
0 1.0 0 rg
0 0 m
200 0 l
200 200 l
0 200 l
h
f";

let mut iter = TypedIter::new(content_stream);
assert!(matches!(iter.next(), Some(TypedInstruction::Transform(_))));
assert!(matches!(iter.next(), Some(TypedInstruction::NonStrokeColorDeviceRgb(_))));
assert!(matches!(iter.next(), Some(TypedInstruction::MoveTo(_))));
assert!(matches!(iter.next(), Some(TypedInstruction::LineTo(_))));
assert!(matches!(iter.next(), Some(TypedInstruction::LineTo(_))));
assert!(matches!(iter.next(), Some(TypedInstruction::LineTo(_))));
assert!(matches!(iter.next(), Some(TypedInstruction::ClosePath(_))));
assert!(matches!(iter.next(), Some(TypedInstruction::FillPathNonZero(_))));
```
*/

#[allow(missing_docs)]
pub mod ops;

use crate::content::ops::TypedInstruction;
use crate::object;
use crate::object::dict::InlineImageDict;
use crate::object::name::{Name, skip_name_like};
use crate::object::{Array, Null, Number, Object, Stream};
use crate::reader::Reader;
use crate::reader::{Readable, ReaderContext, ReaderExt, Skippable};
use crate::trivia::{Comment, is_white_space_character};
use crate::util::find_needle;
use core::array;
use core::fmt::{Debug, Formatter};
use core::ops::Deref;
use core::marker::PhantomData;
use smallvec::SmallVec;

// 6 operands are used for example for ctm or cubic curves,
// but anything above should be pretty rare (only for example for
// DeviceN color spaces, or invalid PDF files). So we settle on 10.
const OPERANDS_THRESHOLD: usize = 10;

impl Debug for Operator<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0.as_str())
    }
}

/// A content stream operator.
#[derive(Clone, PartialEq)]
pub struct Operator<'a>(Name<'a>);

impl Deref for Operator<'_> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Skippable for Operator<'_> {
    fn skip(r: &mut Reader<'_>, _: bool) -> Option<()> {
        skip_name_like(r, false).map(|_| ())
    }
}

impl<'a> Readable<'a> for Operator<'a> {
    fn read(r: &mut Reader<'a>, _: &ReaderContext<'a>) -> Option<Self> {
        let start = r.offset();
        skip_name_like(r, false)?;
        let end = r.offset();
        let data = r.range(start..end)?;

        if data.is_empty() {
            return None;
        }

        Some(Self(Name::new(data)?))
    }
}

/// PdfCraft patch: how many `EI` candidates an inline image examines, and how far past each
/// one the end-of-data heuristic looks, before taking the current candidate. Unbounded, the
/// heuristic re-parsed the rest of the stream for every candidate, and each re-parse ran the
/// heuristic again on the inline images it met: a fuzzed file hung for minutes.
const MAX_EI_CANDIDATES: u32 = 256;
const MAX_EI_LOOKAHEAD_STEPS: u32 = 256;

/// A source of PDF content instructions.
///
/// The tokenizer owns any state needed to assemble one instruction and may choose how bytes are
/// backed. The current production implementation is [`UntypedIter`], which reads one complete
/// byte slice; a streaming implementation can use the same interface without changing typed
/// operator dispatch.
pub trait ContentTokenizer<'a> {
    /// Return the next instruction, or `None` after the content stream ends or becomes invalid.
    fn next_instruction<'b>(&'b mut self) -> Option<Instruction<'b, 'a>>;
}

/// An iterator over operators in the PDF content streams, providing raw access to the
/// instructions. This is the slice-backed [`ContentTokenizer`] implementation.
#[derive(Clone)]
pub struct UntypedIter<'a> {
    reader: Reader<'a>,
    stack: Stack<'a>,
    operator: Option<Operator<'a>>,
    /// PdfCraft patch: this iterator only checks whether data reads as content (the
    /// end-of-data heuristic below); inline images it meets end at their first `EI`.
    lookahead: bool,
    /// PdfCraft patch: whether `reader` holds the whole content stream. The end of a window of a
    /// longer stream can fall inside an instruction, so nothing that reads up to it is decided
    /// (see `needs_more`).
    complete: bool,
    /// PdfCraft patch: set when `next` stopped because the window ran out before the next
    /// instruction was decided. The caller reads more data and resumes at `committed`.
    needs_more: bool,
    /// PdfCraft patch: the offset just after the instruction `next` returned last.
    committed: usize,
}

impl<'a> UntypedIter<'a> {
    /// Create a new untyped iterator.
    pub fn new(data: &'a [u8]) -> Self {
        Self::new_window(data, true)
    }

    /// PdfCraft patch: an iterator over `data`, which is the whole stream when `complete`, and
    /// otherwise the start of a longer one.
    pub(crate) fn new_window(data: &'a [u8], complete: bool) -> Self {
        Self {
            reader: Reader::new(data),
            stack: Stack::new(),
            operator: None,
            lookahead: false,
            complete,
            needs_more: false,
            committed: 0,
        }
    }

    /// Create a new empty untyped iterator.
    pub fn empty() -> Self {
        Self::new_window(&[], true)
    }

    /// Return the next instruction.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<Instruction<'_, 'a>> {
        self.stack.clear();
        self.operator = None;
        self.needs_more = false;
        self.reader.clear_looked_past_end();

        if self.complete {
            self.reader.skip_white_spaces_and_comments();
        } else {
            // A window may drop what comes before the next instruction, blanks included, but not a
            // comment that the window ends in: that comment may go on in the next window.
            self.committed = self.skip_blanks_and_comments();
        }

        while !self.reader.at_end() {
            // I believe booleans/null never appear as an operator?
            if matches!(
                self.reader.peek_byte()?,
                b'/' | b'.' | b'+' | b'-' | b'0'..=b'9' | b'[' | b'<' | b'('
            ) {
                // See issue 994. In all sane scenarios, if the next byte is a number
                // it has to be an operand (a number). However, it's possible that
                // the number is followed by a regular character, in which case it
                // should behave more like an operator (even though there exists
                // no operator that starts with a number). In order to preserve
                // similar behavior to Acrobat and Chromium, we try to consume
                // such an operator and then simply skip it.
                if let Some(object) = self.reader.read_without_context::<Object<'_>>() {
                    if self.undecided() {
                        return None;
                    }
                    self.stack.push(object)?;
                } else if self.reader.read_without_context::<Operator<'_>>().is_some() {
                    if self.undecided() {
                        return None;
                    }
                    self.stack.clear();
                } else {
                    self.undecided();
                    return None;
                }
            } else {
                let operator = match self.reader.read_without_context::<Operator<'_>>() {
                    Some(o) => o,
                    None => {
                        if self.undecided() {
                            return None;
                        }

                        warn!("failed to read operator in content stream");

                        self.reader.jump_to_end();
                        return None;
                    }
                };
                if self.undecided() {
                    return None;
                }

                // Inline images need special casing...
                if operator.as_ref() == b"BI" {
                    // The ID operator will already be consumed by this.
                    let Some(inline_dict) = self
                        .reader
                        .read_without_context::<InlineImageDict<'_>>()
                    else {
                        self.undecided();
                        return None;
                    };
                    let dict = inline_dict.get_dict().clone();

                    // One whitespace after "ID".
                    if self.reader.read_white_space().is_none() {
                        self.undecided();
                        return None;
                    }

                    let stream_data = self.reader.tail()?;
                    let start_offset = self.reader.offset();
                    let mut candidates = 0;
                    // PdfCraft patch: set when a look-ahead below ran into the end of the window.
                    let mut lookahead_touched = false;
                    let mut accepted = false;

                    'outer: while let Some(pos) = find_needle(self.reader.tail()?, b"EI") {
                        self.reader.read_bytes(pos)?;

                        if self.reader.peek_bytes(2) == Some(b"EI") {
                            // If the following character is not a whitespace character, then we are in a ASCII-85 stream.
                            if self
                                .reader
                                .peek_bytes(3)
                                .is_some_and(|b| !is_white_space_character(b[2]))
                            {
                                self.reader.read_bytes(3)?;

                                continue;
                            }

                            let end_offset = self.reader.offset() - start_offset;
                            let image_data = &stream_data[..end_offset];

                            let stream = Stream::new(image_data, dict.clone());

                            // Note that there is a possibility that the encoded stream data
                            // contains the "EI" operator as part of the data, in which case we
                            // cannot confidently know whether we have hit the actual end of the
                            // stream. See also <https://github.com/pdf-association/pdf-issues/issues/543>
                            // PDF 2.0 does have a `/Length` attribute we can read, but since it's relatively
                            // new we don't bother trying to read it.
                            let tail = &self.reader.tail()?[2..];
                            let mut find_reader = Reader::new(tail);
                            // PdfCraft patch: bounded (see `MAX_EI_CANDIDATES`).
                            candidates += 1;
                            let mut steps = 0;
                            let heuristic = !self.lookahead && candidates <= MAX_EI_CANDIDATES;

                            while heuristic && steps < MAX_EI_LOOKAHEAD_STEPS {
                                if find_reader.at_end() {
                                    lookahead_touched = true;
                                    break;
                                }
                                steps += 1;
                                let remaining = find_reader.tail()?;
                                let next_ei = find_needle(remaining, b"EI");
                                let next_bi = find_needle(remaining, b"BI");

                                let (next_pos, is_ei) = match (next_ei, next_bi) {
                                    (Some(ei), Some(bi)) if ei <= bi => (ei, true),
                                    (Some(_), Some(bi)) => (bi, false),
                                    (Some(ei), None) => (ei, true),
                                    (None, Some(bi)) => (bi, false),
                                    (None, None) => {
                                        lookahead_touched = true;
                                        break;
                                    }
                                };

                                find_reader.read_bytes(next_pos)?;

                                if is_ei {
                                    let analyze_data = &tail[..find_reader.offset()];

                                    // If there is any binary data in-between, we for sure
                                    // have not reached the end.
                                    if analyze_data.iter().any(|c| !c.is_ascii()) {
                                        self.reader.read_bytes(2)?;
                                        continue 'outer;
                                    }

                                    // Otherwise, the only possibility that we reached an
                                    // "EI", even though the previous one was valid, is
                                    // that it's part of a string in the content
                                    // stream that follows the inline image. Therefore,
                                    // it should be valid to interpret `tail` as a content
                                    // stream and there should be at least one text-related
                                    // operator that can be parsed correctly.

                                    let mut iter = TypedIter::from_untyped(UntypedIter {
                                        lookahead: true,
                                        ..UntypedIter::new_window(tail, self.complete)
                                    });
                                    let mut found = false;
                                    let mut counter = 0;

                                    while let Some(op) = iter.next() {
                                        // If we have read more than 20 valid operators, it should be
                                        // safe to assume that we are in a content stream, so abort
                                        // early. The only situation where this could reasonably
                                        // be violated is if we have 20 subsequent instances of
                                        // q/Q in the image data, which seems very unlikely.
                                        if counter >= 20 {
                                            found = true;
                                            break;
                                        }

                                        if matches!(
                                            op,
                                            TypedInstruction::NextLineAndShowText(_)
                                                | TypedInstruction::ShowText(_)
                                                | TypedInstruction::ShowTexts(_)
                                                | TypedInstruction::ShowTextWithParameters(_)
                                        ) {
                                            // Now it should be safe to assume that the
                                            // previous `EI` was the correct one.
                                            found = true;
                                            break;
                                        }

                                        counter += 1;
                                    }
                                    lookahead_touched |=
                                        iter.needs_more() || iter.untyped.reader.looked_past_end();

                                    if !found {
                                        // Seems like the data in-between is not a valid content
                                        // stream, so we are likely still within the image data.
                                        self.reader.read_bytes(2)?;
                                        continue 'outer;
                                    }
                                } else {
                                    // Possibly another inline image, if so, the previously found "EI"
                                    // is indeed the end of data.
                                    let mut cloned = find_reader.clone();
                                    cloned.read_bytes(2)?;
                                    let is_inline_image_dict = cloned
                                        .read_without_context::<InlineImageDict<'_>>()
                                        .is_some();
                                    lookahead_touched |= cloned.looked_past_end();
                                    if is_inline_image_dict {
                                        break;
                                    }
                                }

                                find_reader.read_byte()?;
                            }

                            // PdfCraft patch: the end is only certain if no look-ahead needed bytes
                            // past the window. Otherwise read more and decide again.
                            if !self.complete
                                && (lookahead_touched || self.reader.looked_past_end())
                            {
                                self.needs_more = true;
                                return None;
                            }

                            self.stack.push(Object::Stream(stream))?;

                            self.reader.read_bytes(2)?;
                            self.reader.skip_white_spaces();

                            accepted = true;

                            break;
                        }
                    }

                    // PdfCraft patch: without an accepted "EI" the image may still end in later data.
                    if !accepted && !self.complete {
                        self.needs_more = true;
                        return None;
                    }
                }

                self.operator = Some(operator);
                self.committed = self.reader.offset();
                return Some(Instruction {
                    operands: &self.stack,
                    operator: self.operator.as_ref().unwrap(),
                });
            }

            self.reader.skip_white_spaces_and_comments();
        }

        // PdfCraft patch: the window ended between instructions, and another may follow.
        if !self.complete {
            self.needs_more = true;
        }

        None
    }

    /// PdfCraft patch: skips the blanks and comments before an instruction, as
    /// `skip_white_spaces_and_comments` does, and returns where a window may be cut: the start of a
    /// comment that runs into the end of the window, or else where the skipping stopped.
    fn skip_blanks_and_comments(&mut self) -> usize {
        let mut open_comment = None;
        while let Some(b) = self.reader.peek_byte() {
            if is_white_space_character(b) {
                self.reader.skip_white_spaces();
            } else if b == b'%' {
                let start = self.reader.offset();
                Comment::skip(&mut self.reader, true);
                open_comment = self.reader.at_end().then_some(start);
            } else {
                break;
            }
        }

        open_comment.unwrap_or(self.reader.offset())
    }

    /// PdfCraft patch: whether the instruction being read ran into the end of a window, so it
    /// cannot be decided yet. Records that more data is needed.
    fn undecided(&mut self) -> bool {
        if !self.complete && self.reader.looked_past_end() {
            self.needs_more = true;
        }

        self.needs_more
    }
}

impl<'a> ContentTokenizer<'a> for UntypedIter<'a> {
    fn next_instruction<'b>(&'b mut self) -> Option<Instruction<'b, 'a>> {
        self.next()
    }
}


/// An iterator over PDF content streams that provide access to the instructions
/// in a typed fashion.
#[derive(Clone)]
pub struct TypedIter<'a, T = UntypedIter<'a>> {
    untyped: T,
    marker: PhantomData<&'a ()>,
}

impl<'a> TypedIter<'a, UntypedIter<'a>> {
    /// Create a new typed iterator.
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            untyped: UntypedIter::new(data),
            marker: PhantomData,
        }
    }

    /// PdfCraft patch: an iterator over a window of a content stream: the whole stream when
    /// `complete`, and otherwise the start of a longer one (see [`TypedIter::needs_more`]).
    pub fn new_window(data: &'a [u8], complete: bool) -> Self {
        Self {
            untyped: UntypedIter::new_window(data, complete),
            marker: PhantomData,
        }
    }

    /// PdfCraft patch: whether the last `next` returned `None` only because the window ran out
    /// before the next instruction was decided. Read more of the stream, then resume at
    /// [`TypedIter::committed`] in a window that starts there.
    pub fn needs_more(&self) -> bool {
        self.untyped.needs_more
    }

    /// PdfCraft patch: the offset in the window just after the last instruction `next` returned.
    pub fn committed(&self) -> usize {
        self.untyped.committed
    }

    pub(crate) fn from_untyped(untyped: UntypedIter<'a>) -> Self {
        Self { untyped, marker: PhantomData }
    }
}

impl<'a, T> TypedIter<'a, T>
where
    T: ContentTokenizer<'a>,
{
    /// Adapt a content tokenizer to the typed operator interface.
    pub fn from_tokenizer(tokenizer: T) -> Self {
        Self { untyped: tokenizer, marker: PhantomData }
    }

    /// Return the next typed instruction.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<TypedInstruction<'_, 'a>> {
        let op = self.untyped.next_instruction()?;
        // TODO: Explore whether dispatching can be made more efficient.
        match TypedInstruction::dispatch(&op) {
            Some(op) => Some(op),
            // In case this returns `None`, the content stream is invalid. In case a path-drawing
            // operator was used, let's abort completely, otherwise we might end up drawing random stuff.
            // However, for other operators it could be worth it to just skip it but keep attempting
            // to read other content operators.
            None => {
                if [
                    &b"m"[..],
                    &b"l"[..],
                    &b"c"[..],
                    &b"v"[..],
                    &b"y"[..],
                    &b"h"[..],
                    &b"re"[..],
                ]
                .contains(&op.operator.0.deref())
                {
                    None
                } else {
                    Some(TypedInstruction::Fallback(op.operator))
                }
            }
        }
    }
}

/// An instruction (= operator and its operands) in a content stream.
pub struct Instruction<'b, 'a> {
    /// The stack containing the operands.
    pub operands: &'b Stack<'a>,
    /// The actual operator.
    pub operator: &'b Operator<'a>,
}

impl<'b, 'a> Instruction<'b, 'a> {
    /// An iterator over the operands of the instruction.
    pub fn operands(&self) -> OperandIterator<'b, 'a> {
        OperandIterator::new(self.operands)
    }
}

/// A stack holding the arguments of an operator.
pub struct Stack<'a> {
    // TODO: Explore using an object pool to avoid repeatedly
    // allocating/deallocating objects.
    data: [Object<'a>; OPERANDS_THRESHOLD],
    len: usize,
}

impl<'a> Default for Stack<'a> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> Stack<'a> {
    /// Create a new, empty stack.
    pub fn new() -> Self {
        Self {
            data: array::from_fn(|_| Object::Null(Null)),
            len: 0,
        }
    }

    fn push(&mut self, operand: Object<'a>) -> Option<()> {
        if self.len >= OPERANDS_THRESHOLD {
            return None;
        }

        self.data[self.len] = operand;
        self.len += 1;
        Some(())
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    fn len(&self) -> usize {
        self.len
    }

    fn as_slice(&self) -> &[Object<'a>] {
        &self.data[..self.len]
    }

    fn get<'b, T>(&'b self, index: usize) -> Option<T>
    where
        T: Operand<'b, 'a>,
    {
        self.as_slice().get(index).and_then(T::from_object)
    }

    fn get_all<'b, T>(&'b self) -> Option<SmallVec<[T; OPERANDS_THRESHOLD]>>
    where
        T: Operand<'b, 'a>,
    {
        let mut operands = SmallVec::new();

        for op in self.as_slice() {
            let converted = T::from_object(op)?;
            operands.push(converted);
        }

        Some(operands)
    }
}

impl Debug for Stack<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.as_slice()).finish()
    }
}

impl Clone for Stack<'_> {
    fn clone(&self) -> Self {
        let mut stack = Self::new();
        for item in self.as_slice() {
            stack.push(item.clone()).unwrap();
        }
        stack
    }
}

impl PartialEq for Stack<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

trait Operand<'b, 'a>: Sized {
    fn from_object(object: &'b Object<'a>) -> Option<Self>;
}

impl<'b, 'a> Operand<'b, 'a> for Number {
    fn from_object(object: &'b Object<'a>) -> Option<Self> {
        match object {
            Object::Number(n) => Some(*n),
            _ => None,
        }
    }
}

impl<'b, 'a> Operand<'b, 'a> for &'b object::String<'a> {
    fn from_object(object: &'b Object<'a>) -> Option<Self> {
        match object {
            Object::String(s) => Some(s),
            _ => None,
        }
    }
}

impl<'b, 'a> Operand<'b, 'a> for &'b Name<'a> {
    fn from_object(object: &'b Object<'a>) -> Option<Self> {
        match object {
            Object::Name(n) => Some(n),
            _ => None,
        }
    }
}

impl<'b, 'a> Operand<'b, 'a> for &'b Array<'a> {
    fn from_object(object: &'b Object<'a>) -> Option<Self> {
        match object {
            Object::Array(a) => Some(a),
            _ => None,
        }
    }
}

impl<'b, 'a> Operand<'b, 'a> for &'b Stream<'a> {
    fn from_object(object: &'b Object<'a>) -> Option<Self> {
        match object {
            Object::Stream(s) => Some(s),
            _ => None,
        }
    }
}

impl<'b, 'a> Operand<'b, 'a> for &'b Object<'a> {
    fn from_object(object: &'b Object<'a>) -> Option<Self> {
        Some(object)
    }
}

/// An iterator over the operands of an operator.
pub struct OperandIterator<'b, 'a> {
    stack: &'b Stack<'a>,
    cur_index: usize,
}

impl<'b, 'a> OperandIterator<'b, 'a> {
    fn new(stack: &'b Stack<'a>) -> Self {
        Self {
            stack,
            cur_index: 0,
        }
    }
}

impl<'b, 'a> Iterator for OperandIterator<'b, 'a> {
    type Item = &'b Object<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(item) = self.stack.as_slice().get(self.cur_index) {
            self.cur_index += 1;

            Some(item)
        } else {
            None
        }
    }
}

pub(crate) trait OperatorTrait<'b, 'a>: Sized {
    const OPERATOR: &'static str;

    fn from_stack(stack: &'b Stack<'a>) -> Option<Self>;
}

mod macros {
    macro_rules! op_impl {
        ($t:ident $(<$($l:lifetime),+>)?, $e:expr, $n:expr, |$stack:ident : $stack_ty:ty| $body:block) => {
            impl<'b, 'a> OperatorTrait<'b, 'a> for $t$(<$($l),+>)? {
                const OPERATOR: &'static str = $e;

                #[inline(always)]
                fn from_stack($stack: $stack_ty) -> Option<Self> {
                    $body.or_else(|| {
                        warn!("failed to convert operands for operator {}", Self::OPERATOR);

                        None
                    })
                }
            }

            impl<'b, 'a> From<$t$(<$($l),+>)?> for TypedInstruction<'b, 'a> {
                fn from(value: $t$(<$($l),+>)?) -> Self {
                    TypedInstruction::$t(value)
                }
            }

            impl<'b, 'a> TryFrom<TypedInstruction<'b, 'a>> for $t$(<$($l),+>)? {
                type Error = ();

                fn try_from(value: TypedInstruction<'b, 'a>) -> core::result::Result<Self, Self::Error> {
                    match value {
                        TypedInstruction::$t(e) => Ok(e),
                        _ => Err(())
                    }
                }
            }
        };
    }

    // The `shift` parameter will always be 0 in valid PDFs. The purpose of the parameter is
    // so that in case there are garbage operands in the content stream, we prefer to use
    // the operands that are closer to the operator instead of the values at the bottom
    // of the stack.

    macro_rules! op0 {
        ($t:ident $(<$($l:lifetime),+>)?, $e:expr) => {
            crate::content::macros::op_impl!($t$(<$($l),+>)?, $e, 0, |_stack: &'b Stack<'a>| {
                Some(Self)
            });
        }
    }

    macro_rules! op1 {
        ($t:ident $(<$($l:lifetime),+>)?, $e:expr) => {
            crate::content::macros::op_impl!($t$(<$($l),+>)?, $e, 1, |stack: &'b Stack<'a>| {
                let shift = stack.len().saturating_sub(1);
                Some(Self(stack.get(0 + shift)?))
            });
        }
    }

    macro_rules! op_all {
        ($t:ident $(<$($l:lifetime),+>)?, $e:expr) => {
            crate::content::macros::op_impl!($t$(<$($l),+>)?, $e, u8::MAX as usize, |stack: &'b Stack<'a>| {
                Some(Self(stack.get_all()?))
            });
        }
    }

    macro_rules! op2 {
        ($t:ident $(<$($l:lifetime),+>)?, $e:expr) => {
            crate::content::macros::op_impl!($t$(<$($l),+>)?, $e, 2, |stack: &'b Stack<'a>| {
                let shift = stack.len().saturating_sub(2);
                Some(Self(stack.get(0 + shift)?, stack.get(1 + shift)?))
            });
        }
    }

    macro_rules! op3 {
        ($t:ident $(<$($l:lifetime),+>)?, $e:expr) => {
            crate::content::macros::op_impl!($t$(<$($l),+>)?, $e, 3, |stack: &'b Stack<'a>| {
                let shift = stack.len().saturating_sub(3);
                Some(Self(stack.get(0 + shift)?, stack.get(1 + shift)?,
                stack.get(2 + shift)?))
            });
        }
    }

    macro_rules! op4 {
        ($t:ident $(<$($l:lifetime),+>)?, $e:expr) => {
            crate::content::macros::op_impl!($t$(<$($l),+>)?, $e, 4, |stack: &'b Stack<'a>| {
               let shift = stack.len().saturating_sub(4);
            Some(Self(stack.get(0 + shift)?, stack.get(1 + shift)?,
            stack.get(2 + shift)?, stack.get(3 + shift)?))
            });
        }
    }

    macro_rules! op6 {
        ($t:ident $(<$($l:lifetime),+>)?, $e:expr) => {
            crate::content::macros::op_impl!($t$(<$($l),+>)?, $e, 6, |stack: &'b Stack<'a>| {
                let shift = stack.len().saturating_sub(6);
            Some(Self(stack.get(0 + shift)?, stack.get(1 + shift)?,
            stack.get(2 + shift)?, stack.get(3 + shift)?,
            stack.get(4 + shift)?, stack.get(5 + shift)?))
            });
        }
    }

    pub(crate) use op_all;
    pub(crate) use op_impl;
    pub(crate) use op0;
    pub(crate) use op1;
    pub(crate) use op2;
    pub(crate) use op3;
    pub(crate) use op4;
    pub(crate) use op6;
}

#[cfg(test)]
mod tests {
    use super::{ContentTokenizer, UntypedIter};

    fn operators(data: &[u8]) -> Vec<Vec<u8>> {
        let mut tokenizer = UntypedIter::new(data);
        let mut result = Vec::new();
        while let Some(instruction) = tokenizer.next_instruction() {
            result.push(instruction.operator.as_ref().to_vec());
        }
        result
    }

    #[test]
    fn slice_tokenizer_keeps_the_existing_operator_sequence() {
        let data = b"% comment\n1 -2.5 /Name /A#20B (a\\(b\\) \\101) <4869> [1 2] << /Key (value) >> [(a) 1] TJ BI /W 1 /H 1 /BPC 8 /CS /DeviceGray ID \x00 EI q Q";

        assert_eq!(operators(data), [b"TJ".to_vec(), b"BI".to_vec(), b"q".to_vec(), b"Q".to_vec()]);
    }

    #[test]
    fn tokenizer_interface_preserves_every_byte_boundary() {
        let data = b"% comment\n1 -2.5 /Name /A#20B (a\\(b\\) \\101) <4869> [1 2] << /Key (value) >> [(a) 1] TJ BI /W 1 /H 1 /BPC 8 /CS /DeviceGray ID \x00 EI q Q";
        let expected = operators(data);

        for split in 1..data.len() {
            let mut joined = Vec::with_capacity(data.len());
            joined.extend_from_slice(&data[..split]);
            joined.extend_from_slice(&data[split..]);
            assert_eq!(operators(&joined), expected, "split at byte {split}");
        }
    }

    /// The instructions of `data`, read from the whole of it.
    fn slice_instructions(data: &[u8]) -> Vec<String> {
        let mut tokenizer = UntypedIter::new(data);
        let mut result = Vec::new();
        while let Some(instruction) = tokenizer.next_instruction() {
            result.push(format!("{:?} {:?}", instruction.operator, instruction.operands));
        }
        result
    }

    /// The instructions of `data` as a window driver reads them: the stream arrives in pieces of
    /// the lengths `next_piece` gives. Each window holds the bytes from the first instruction
    /// that is not yet decided, and another piece is appended when a window runs out.
    fn windowed_instructions(data: &[u8], mut next_piece: impl FnMut() -> usize) -> Vec<String> {
        let mut result = Vec::new();
        let mut window: Vec<u8> = Vec::new();
        let mut fed = 0;
        let mut exhausted = false;
        loop {
            let (committed, needs_more) = {
                let mut tokenizer = UntypedIter::new_window(&window, exhausted);
                while let Some(instruction) = tokenizer.next_instruction() {
                    result.push(format!("{:?} {:?}", instruction.operator, instruction.operands));
                }
                (tokenizer.committed, tokenizer.needs_more)
            };
            window.drain(..committed);
            if !needs_more || exhausted {
                break;
            }
            if fed == data.len() {
                exhausted = true;
                continue;
            }
            let end = fed.saturating_add(next_piece().max(1)).min(data.len());
            window.extend_from_slice(&data[fed..end]);
            fed = end;
        }

        result
    }

    /// Content whose instructions are hard to read in pieces: inline images with `EI` in their
    /// data or right after it, unterminated tokens, and operands that are too many or invalid.
    const WINDOWED_CORPUS: [&[u8]; 14] = [
        b"% comment\n1 -2.5 /Name /A#20B (a\\(b\\) \\101) <4869> [1 2] << /Key (value) >> [(a) 1] TJ BI /W 1 /H 1 /BPC 8 /CS /DeviceGray ID \x00 EI q Q",
        b"BT /F1 12 Tf 10 20 Td [(Hel) -250 (lo)] TJ (x) Tj ET",
        b"q 1 0 0 1 0 0 cm BI /W 2 /H 2 /BPC 8 /CS /G ID \x00\x01EI\x00 EI Q 0 0 m 1 1 l S",
        b"BI /W 1 /H 1 /BPC 8 /CS /G ID EI x EI EI BT (a) Tj ET",
        b"0.5 .5 -.5 +5 12.3.4 f",
        b"/A B C /D 1 2 3 4 5 6 7 8 9 10 11 12 re",
        b"BI /W 1 /H 1 /BPC 8 /CS /G ID\n\x00\nEI BI /W 1 /H 1 /BPC 8 /CS /G ID \x01 EI Q",
        b"(unterminated string 1 0 0 1 0 0 cm",
        b"1 0 0 1 0 0 cm\n%comment at the end",
        b"BI /W 1 /H 1 /BPC 8 /CS /G ID \x00 EIxy q",
        b"BI /W 2 /H 1 /BPC 8 /CS /G ID \x00\x01 EI \x00 EI \x01 q",
        b"[1 [2 [3]]] 0 d q Q",
        b"/Lbl BDC EMC 1 w",
        b"<</A 1>> BDC BI /W 1 /H 1 /BPC 8 /CS /G ID \x07 EI (end) Tj",
    ];

    #[test]
    fn windows_keep_every_instruction_at_every_split() {
        for data in WINDOWED_CORPUS {
            let expected = slice_instructions(data);
            for split in 1..=data.len() {
                let mut lengths = [split, data.len()].into_iter();
                let windowed = windowed_instructions(data, || lengths.next().unwrap_or(data.len()));
                assert_eq!(windowed, expected, "{:?} split at byte {split}", String::from_utf8_lossy(data));
            }
        }
    }

    #[test]
    fn windows_keep_every_instruction_in_pieces_of_any_size() {
        for data in WINDOWED_CORPUS {
            let expected = slice_instructions(data);
            for piece in 1..=data.len() {
                let windowed = windowed_instructions(data, || piece);
                assert_eq!(windowed, expected, "{:?} in pieces of {piece}", String::from_utf8_lossy(data));
            }
        }
    }

    #[test]
    fn windows_drop_blanks_and_keep_a_comment_that_may_go_on() {
        // Blanks alone decide nothing, so a window of them can be dropped whole.
        let blanks = vec![b' '; 4096];
        let mut tokenizer = UntypedIter::new_window(&blanks, false);
        assert!(tokenizer.next().is_none());
        assert!(tokenizer.needs_more);
        assert_eq!(tokenizer.committed, blanks.len());

        // The window ends in a comment: it is kept, from its `%`, for the next window.
        let data = b"1 0 0 1 0 0 cm  % a comment that goes on";
        let mut tokenizer = UntypedIter::new_window(data, false);
        assert!(tokenizer.next().is_some());
        assert!(tokenizer.next().is_none());
        assert!(tokenizer.needs_more);
        assert_eq!(tokenizer.committed, data.iter().position(|&b| b == b'%').expect("a comment"));
    }

    #[test]
    fn windows_match_the_slice_on_random_content() {
        const PARTS: [&[u8]; 24] = [
            b"BI", b" ID ", b"EI", b" EI ", b"/W", b" 1 ", b"-2.5", b".5", b"(", b")", b"\\", b"<",
            b">", b"[", b"]", b"TJ", b"Tj", b"q", b"Q", b"re", b"f", b"% c\n", b"\n", b"\x00\xff",
        ];
        // A fixed xorshift sequence, so that a failure can be reproduced.
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };

        for _ in 0..2000 {
            let mut data = Vec::new();
            for _ in 0..next() % 40 {
                let part = PARTS[(next() % PARTS.len() as u64) as usize];
                data.extend_from_slice(part);
            }

            let expected = slice_instructions(&data);
            let windowed = windowed_instructions(&data, || 1 + (next() % 12) as usize);
            assert_eq!(windowed, expected, "{:?}", String::from_utf8_lossy(&data));
        }
    }
}
