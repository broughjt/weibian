use ecow::EcoVec;
use typst::{
    diag::{Severity, SourceDiagnostic, Warned},
    World,
};
use typst_html::HtmlDocument;

const HTML_MESSAGE: &str = "html export is under active development and incomplete";

pub fn compile<W: World>(
    world: &W,
) -> Warned<Result<(HtmlDocument, String), EcoVec<SourceDiagnostic>>> {
    let Warned {
        output: result,
        mut warnings,
    } = typst::compile::<HtmlDocument>(&world);

    let keep =
        |d: &mut SourceDiagnostic| !(d.severity == Severity::Warning && d.message == HTML_MESSAGE);

    warnings.retain(keep);

    match result {
        Ok(document) => Warned {
            output: typst_html::html(&document).map(|content| (document, content)),
            warnings,
        },
        Err(mut errors) => {
            errors.retain(keep);

            Warned {
                output: Err(errors),
                warnings,
            }
        }
    }
}
