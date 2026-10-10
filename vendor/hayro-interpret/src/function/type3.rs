use crate::function::{Clamper, Function, TupleVec, Values, interpolate};
use hayro_syntax::object::Array;
use hayro_syntax::object::Dict;
use hayro_syntax::object::Object;
use hayro_syntax::object::dict::keys::{BOUNDS, ENCODE, FUNCTIONS};
use smallvec::smallvec;

/// A type 3 function (stitching function).
#[derive(Debug)]
pub(crate) struct Type3 {
    functions: Vec<Function>,
    bounds: Vec<f32>,
    encode: TupleVec,
    clamper: Clamper,
}

impl Type3 {
    /// Create a new type 3 function.
    pub(super) fn new(
        dict: &Dict<'_>,
        budget: &mut super::ConstructionBudget,
        depth: usize,
    ) -> Option<Self> {
        let clamper = Clamper::new(dict)?;

        // PdfCraft patch: all descendants share the caller's depth, work and cycle guards.
        // The resolved iterator also ends on an unresolved/cyclic reference. Require one
        // resolved child per raw slot so that failure cannot silently shorten the function list.
        let array = dict.get::<Array<'_>>(FUNCTIONS)?;
        let mut objects = array.iter::<Object<'_>>();
        let functions: Vec<Function> = array
            .raw_iter()
            .map(|_| {
                let Some(object) = objects.next() else {
                    warn!(
                        "PDF stitching function contains an unresolved or cyclic child reference"
                    );
                    return None;
                };
                Function::new_inner(&object, budget, depth)
            })
            .collect::<Option<_>>()?;
        if functions.is_empty() {
            warn!("PDF stitching function has no child functions");
            return None;
        }
        let domain = *clamper.domain.first()?;
        let mut bounds = vec![domain.0 - 0.0001];
        if let Some(a) = dict.get::<Array<'_>>(BOUNDS) {
            bounds.extend(a.iter::<f32>());
        }
        // Add a small delta so that the interval is considered to be closed on the right.
        bounds.push(domain.1 + 0.0001);

        let encode = dict.get::<TupleVec>(ENCODE)?;

        Some(Self {
            functions,
            clamper,
            bounds,
            encode,
        })
    }

    /// Evaluate the function with the given input.
    pub(crate) fn eval(&self, input: f32) -> Option<Values> {
        let mut input = [input];
        self.clamper.clamp_input(&mut input);

        let index = find_interval(&self.bounds, input[0])?;

        let bounds_i = *self.bounds.get(index + 1)?;
        let bounds_i_minus_1 = *self.bounds.get(index)?;

        // - 1 because we inserted a dummy bound in the constructor.
        let encoding = self.encode.get(index)?;
        let function = self.functions.get(index)?;
        let encoded = interpolate(input[0], bounds_i_minus_1, bounds_i, encoding.0, encoding.1);

        let mut evaluated = function.eval(smallvec![encoded])?;

        self.clamper.clamp_output(&mut evaluated);

        Some(evaluated)
    }
}

fn find_interval(bounds: &[f32], x: f32) -> Option<usize> {
    if x < *bounds.first()? || x >= *bounds.last()? {
        return None;
    }

    match bounds.binary_search_by(|val| {
        if *val <= x {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        }
    }) {
        Ok(i) => Some(i - 1),
        Err(i) => Some(i - 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hayro_syntax::object::FromBytes;

    #[test]
    fn simple() {
        let data = b"<<
  /FunctionType 3
  /Domain [-7 7]
  /Functions [
    << /FunctionType 2
       /Domain [0 1]
       /C0 [0.5 0.5 0.5]
       /C1 [0.5 0.5 0.5]
       /N 1
    >>
    << /FunctionType 2
       /Domain [0 1]
       /C0 [0.7 0.7 0.7]
       /C1 [0.7 0.7 0.7]
       /N 1
    >>
  ]
  /Bounds [0]
  /Encode [0 1 0 1]
>>";

        let dict = Object::from_bytes(data).unwrap();
        let function = Function::new(&dict).unwrap();

        assert_eq!(
            function.eval(smallvec![-7.0]).unwrap().as_slice(),
            &[0.5, 0.5, 0.5]
        );
        assert_eq!(
            function.eval(smallvec![-3.0]).unwrap().as_slice(),
            &[0.5, 0.5, 0.5]
        );
        assert_eq!(
            function.eval(smallvec![-0.5]).unwrap().as_slice(),
            &[0.5, 0.5, 0.5]
        );
        assert_eq!(
            function.eval(smallvec![0.0]).unwrap().as_slice(),
            &[0.7, 0.7, 0.7]
        );
        assert_eq!(
            function.eval(smallvec![7.0]).unwrap().as_slice(),
            &[0.7, 0.7, 0.7]
        );
    }

    fn function_limits_stitching(levels: usize) -> String {
        let mut function = "<< /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>".to_owned();
        for _ in 1..levels {
            function = format!(
                "<< /FunctionType 3 /Domain [0 1] /Functions [{function}] /Bounds [] /Encode [0 1] >>"
            );
        }
        function
    }

    #[test]
    fn function_limits_stitching_depth_is_bounded() {
        let data = function_limits_stitching(64);
        let object = Object::from_bytes(data.as_bytes()).unwrap();
        assert_eq!(
            Function::new(&object).unwrap().eval(smallvec![0.5]),
            Some(smallvec![0.5])
        );
        let data = function_limits_stitching(65);
        // Direct nesting beyond the hayro-syntax cap (PdfCraft patch) is refused while parsing,
        // before this guard; the guard is tested with indirect references in pdfcraft-render
        // (`function_limits_depth_guard_refuses_a_chain_of_references`).
        assert!(Object::from_bytes(data.as_bytes()).is_none());
    }

    fn function_limits_fanout(children: usize) -> String {
        let leaf = "<< /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> ";
        format!(
            "<< /FunctionType 3 /Domain [0 1] /Functions [{}] /Bounds [{}] /Encode [{}] >>",
            leaf.repeat(children),
            "0.5 ".repeat(children.saturating_sub(1)),
            "0 1 ".repeat(children)
        )
    }

    #[test]
    fn function_limits_stitching_fanout_shares_a_node_budget() {
        let data = function_limits_fanout(9999);
        let object = Object::from_bytes(data.as_bytes()).unwrap();
        assert!(Function::new(&object).is_some());
        let data = function_limits_fanout(10000);
        let object = Object::from_bytes(data.as_bytes()).unwrap();
        assert!(Function::new(&object).is_none());
    }
}
