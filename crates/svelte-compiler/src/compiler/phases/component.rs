use std::marker::PhantomData;

use crate::api::CompileOptions;
use crate::ast::modern::Root;
use crate::compiler::phases::parse::ParsedComponent;
use crate::{SourceId, SourceText};

#[derive(Debug, Clone)]
pub(crate) struct ComponentContext<'a> {
    parsed: ParsedComponent,
    options: &'a CompileOptions,
}

impl<'a> ComponentContext<'a> {
    pub(crate) fn new(parsed: ParsedComponent, options: &'a CompileOptions) -> Self {
        Self { parsed, options }
    }

    pub(crate) fn source(&self) -> &str {
        self.parsed.source()
    }

    pub(crate) fn options(&self) -> &'a CompileOptions {
        self.options
    }

    pub(crate) fn root(&self) -> &Root {
        self.parsed.root()
    }

    pub(crate) fn source_text(&self) -> SourceText<'_> {
        SourceText::new(
            SourceId::new(0),
            self.source(),
            self.options.filename.as_deref(),
        )
    }
}

impl AsRef<str> for ComponentContext<'_> {
    fn as_ref(&self) -> &str {
        self.source()
    }
}

impl AsRef<CompileOptions> for ComponentContext<'_> {
    fn as_ref(&self) -> &CompileOptions {
        self.options()
    }
}

impl AsRef<Root> for ComponentContext<'_> {
    fn as_ref(&self) -> &Root {
        self.root()
    }
}

mod sealed {
    pub trait Sealed {}
}

pub(crate) trait ComponentStage: sealed::Sealed {}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Analyzed;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Lowered;

impl sealed::Sealed for Analyzed {}
impl sealed::Sealed for Lowered {}

impl ComponentStage for Analyzed {}
impl ComponentStage for Lowered {}

#[derive(Debug, Clone)]
pub(crate) struct ComponentCompilation<'a, Stage: ComponentStage> {
    ctx: ComponentContext<'a>,
    runes: bool,
    _stage: PhantomData<Stage>,
}

pub(crate) type ComponentAnalysis<'a> = ComponentCompilation<'a, Analyzed>;
pub(crate) type LoweredComponent<'a> = ComponentCompilation<'a, Lowered>;

impl<'a, Stage: ComponentStage> ComponentCompilation<'a, Stage> {
    fn new(ctx: ComponentContext<'a>, runes: bool) -> Self {
        Self {
            ctx,
            runes,
            _stage: PhantomData,
        }
    }

    pub(crate) fn source(&self) -> &str {
        self.ctx.source()
    }

    pub(crate) fn options(&self) -> &'a CompileOptions {
        self.ctx.options()
    }

    pub(crate) fn root(&self) -> &Root {
        self.ctx.root()
    }

    pub(crate) fn source_text(&self) -> SourceText<'_> {
        self.ctx.source_text()
    }

    pub(crate) fn runes(&self) -> bool {
        self.runes
    }
}

impl<'a> ComponentAnalysis<'a> {
    pub(crate) fn from_context(ctx: ComponentContext<'a>, runes: bool) -> Self {
        Self::new(ctx, runes)
    }

    pub(crate) fn lower(self) -> LoweredComponent<'a> {
        LoweredComponent::new(self.ctx, self.runes)
    }
}

impl<Stage: ComponentStage> AsRef<str> for ComponentCompilation<'_, Stage> {
    fn as_ref(&self) -> &str {
        self.source()
    }
}

impl<Stage: ComponentStage> AsRef<CompileOptions> for ComponentCompilation<'_, Stage> {
    fn as_ref(&self) -> &CompileOptions {
        self.options()
    }
}

impl<Stage: ComponentStage> AsRef<Root> for ComponentCompilation<'_, Stage> {
    fn as_ref(&self) -> &Root {
        self.root()
    }
}
