use svelte_kit::{Error, SyntaxError, parse_module_syntax};

#[test]
fn malformed_client_module_reports_upstream_parser_error() {
    let error =
        parse_module_syntax("export const broken = {").expect_err("malformed module should fail");

    assert!(matches!(
        error,
        Error::Syntax(SyntaxError::ParseModule { .. })
    ));
    assert!(
        error.to_string().contains("Unexpected end of input"),
        "received unexpected exception message {}",
        error
    );
}
