#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompilerSuite {
    ParserModern,
    ParserLegacy,
    CompilerErrors,
    Css,
    Migrate,
    Preprocess,
    Print,
    Validator,
    Snapshot,
    Sourcemaps,
}

impl CompilerSuite {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ParserModern => "parser-modern",
            Self::ParserLegacy => "parser-legacy",
            Self::CompilerErrors => "compiler-errors",
            Self::Css => "css",
            Self::Migrate => "migrate",
            Self::Preprocess => "preprocess",
            Self::Print => "print",
            Self::Validator => "validator",
            Self::Snapshot => "snapshot",
            Self::Sourcemaps => "sourcemaps",
        }
    }
}
