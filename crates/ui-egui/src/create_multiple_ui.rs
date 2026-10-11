//! Create a PDF ▸ Multiple files: the picked PDFs, images and text files become one new,
//! unsaved document, shown in the page grid: remove pages, insert more files between pages,
//! reorder, then save what the grid shows.

use std::sync::Arc;

use pdfcraft_engine::EditError;

use crate::PdfCraftApp;

fn stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

/// Why Create Multiple stopped early: a picked file couldn't be read, or the PDF couldn't be made.
enum Stop {
    Read { name: String, error: String },
    Build(EditError),
}

impl From<EditError> for Stop {
    fn from(e: EditError) -> Self {
        Stop::Build(e)
    }
}

impl PdfCraftApp {
    /// Convert the picked files, join them in the order picked, and open the result in the page
    /// grid. A file that can't be converted is left out (and named in a notice); a file that can't be
    /// read stops the run. Files are read as their turn comes, so one file's bytes are held at a time.
    pub(crate) fn stage_create_multiple<I>(&mut self, files: I)
    where
        I: IntoIterator<Item = Result<(String, Vec<u8>), (String, String)>>,
    {
        let (mut refused, mut converted) = (Vec::new(), 0usize);
        let mut files = files.into_iter();
        let session = &self.session;
        let sources = std::iter::from_fn(|| {
            loop {
                match files.next()? {
                    Ok((name, bytes)) => match session.convert_to_pdf(&name, &Arc::new(bytes)) {
                        Ok((_, pdf)) => {
                            converted += 1;
                            return Some(Ok((stem(&name).to_string(), pdf, None)));
                        }
                        Err(_) => refused.push(name),
                    },
                    Err((name, error)) => return Some(Err(Stop::Read { name, error })),
                }
            }
        });
        let bytes = match session.combine_each(sources) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                self.notify_tr("None of those files can be made into a PDF; use PDFs, images or .txt files");
                return;
            }
            Err(Stop::Read { name, error }) => {
                self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &error)]);
                return;
            }
            Err(Stop::Build(e)) => {
                self.notify_fmt("Couldn't create a PDF: {e}", &[("e", &e.to_string())]);
                return;
            }
        };
        let n = converted.to_string();
        let message = if !refused.is_empty() {
            crate::i18n::fmt(tl!("Created a PDF from {n} files; left out: {names}"), &[("n", &n), ("names", &refused.join(", "))])
        } else {
            crate::i18n::fmt(tl!("Created a PDF from {n} files"), &[("n", &n)])
        };
        let before = self.views.len();
        self.open_created("Combined.pdf", bytes, &message);
        if self.views.len() > before
            && let Some(view) = self.active.and_then(|i| self.views.get_mut(i))
        {
            view.organize = true;
        }
    }
}
