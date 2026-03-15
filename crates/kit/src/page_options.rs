use std::collections::BTreeMap;
use std::fs;

use camino::Utf8Path;
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    BindingPattern, Declaration, Expression, ModuleExportName, ObjectPropertyKind, PropertyKey,
    Statement, VariableDeclarator,
};
use oxc_parser::Parser;
use oxc_span::SourceType;
use serde_json::{Number, Value};

pub type PageOptions = BTreeMap<String, Value>;

const VALID_PAGE_OPTIONS: &[&str] = &[
    "ssr",
    "prerender",
    "csr",
    "trailingSlash",
    "config",
    "entries",
    "load",
];

#[derive(Debug, Clone, PartialEq)]
enum AnalyzedValue {
    Static(Value),
    Dynamic,
}

pub fn statically_analyze_page_options(source: &str) -> Option<PageOptions> {
    let allocator = Allocator::default();
    let source_type = SourceType::ts().with_module(true);
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if !parsed.errors.is_empty() {
        return None;
    }

    let mut declarations = BTreeMap::<String, AnalyzedValue>::new();
    let mut options = PageOptions::new();

    for statement in &parsed.program.body {
        match statement {
            Statement::ExportDefaultDeclaration(_) => return None,
            Statement::ExportAllDeclaration(declaration) => {
                let Some(exported) = declaration.exported.as_ref().and_then(module_export_name)
                else {
                    return None;
                };
                if is_valid_page_option(&exported) {
                    return None;
                }
            }
            Statement::ExportNamedDeclaration(declaration) => {
                if declaration.source.is_some() {
                    for specifier in &declaration.specifiers {
                        let Some(exported) = module_export_name(&specifier.exported) else {
                            return None;
                        };
                        if is_valid_page_option(&exported) {
                            return None;
                        }
                    }
                    continue;
                }

                if let Some(inner) = declaration.declaration.as_ref() {
                    analyze_declaration(inner, true, &mut declarations, &mut options)?;
                    continue;
                }

                for specifier in &declaration.specifiers {
                    let Some(exported) = module_export_name(&specifier.exported) else {
                        return None;
                    };
                    if !is_valid_page_option(&exported) {
                        continue;
                    }

                    let Some(local) = module_export_name(&specifier.local) else {
                        return None;
                    };
                    let Some(value) = declarations.get(&local) else {
                        return None;
                    };

                    match value {
                        AnalyzedValue::Static(value) => {
                            options.insert(exported, value.clone());
                        }
                        AnalyzedValue::Dynamic => return None,
                    }
                }
            }
            Statement::VariableDeclaration(declaration) => {
                analyze_variable_declaration(declaration, false, &mut declarations, &mut options)?;
            }
            Statement::FunctionDeclaration(declaration) => {
                analyze_function_declaration(
                    declaration
                        .id
                        .as_ref()
                        .map(|id| id.name.as_str().to_string()),
                    false,
                    &mut declarations,
                    &mut options,
                )?;
            }
            Statement::ClassDeclaration(declaration) => {
                analyze_named_declaration(
                    declaration
                        .id
                        .as_ref()
                        .map(|id| id.name.as_str().to_string()),
                    false,
                    &mut declarations,
                    &mut options,
                    false,
                )?;
            }
            _ => {}
        }
    }

    Some(options)
}

pub fn read_page_options(path: &Utf8Path) -> Option<PageOptions> {
    let source = fs::read_to_string(path).ok()?;
    statically_analyze_page_options(&source)
}

fn analyze_declaration(
    declaration: &Declaration<'_>,
    exported: bool,
    declarations: &mut BTreeMap<String, AnalyzedValue>,
    options: &mut PageOptions,
) -> Option<()> {
    match declaration {
        Declaration::VariableDeclaration(declaration) => {
            analyze_variable_declaration(declaration, exported, declarations, options)
        }
        Declaration::FunctionDeclaration(declaration) => analyze_function_declaration(
            declaration
                .id
                .as_ref()
                .map(|id| id.name.as_str().to_string()),
            exported,
            declarations,
            options,
        ),
        Declaration::ClassDeclaration(declaration) => analyze_named_declaration(
            declaration
                .id
                .as_ref()
                .map(|id| id.name.as_str().to_string()),
            exported,
            declarations,
            options,
            false,
        ),
        _ => Some(()),
    }
}

fn analyze_variable_declaration(
    declaration: &oxc_ast::ast::VariableDeclaration<'_>,
    exported: bool,
    declarations: &mut BTreeMap<String, AnalyzedValue>,
    options: &mut PageOptions,
) -> Option<()> {
    for declarator in &declaration.declarations {
        analyze_variable_declarator(declarator, exported, declarations, options)?;
    }

    Some(())
}

fn analyze_variable_declarator(
    declarator: &VariableDeclarator<'_>,
    exported: bool,
    declarations: &mut BTreeMap<String, AnalyzedValue>,
    options: &mut PageOptions,
) -> Option<()> {
    let Some(name) = binding_name(&declarator.id) else {
        if exported {
            return None;
        }
        return Some(());
    };

    let value = analyze_initializer(&name, declarator.init.as_ref());
    declarations.insert(name.to_string(), value.clone());

    if exported && is_valid_page_option(&name) {
        match value {
            AnalyzedValue::Static(value) => {
                options.insert(name.to_string(), value);
            }
            AnalyzedValue::Dynamic => return None,
        }
    }

    Some(())
}

fn analyze_function_declaration(
    name: Option<String>,
    exported: bool,
    declarations: &mut BTreeMap<String, AnalyzedValue>,
    options: &mut PageOptions,
) -> Option<()> {
    analyze_named_declaration(name, exported, declarations, options, true)
}

fn analyze_named_declaration(
    name: Option<String>,
    exported: bool,
    declarations: &mut BTreeMap<String, AnalyzedValue>,
    options: &mut PageOptions,
    is_function: bool,
) -> Option<()> {
    let Some(name) = name else {
        return Some(());
    };

    let value = if name == "load" && is_function {
        AnalyzedValue::Static(Value::Null)
    } else {
        AnalyzedValue::Dynamic
    };
    declarations.insert(name.clone(), value.clone());

    if exported && is_valid_page_option(&name) {
        match value {
            AnalyzedValue::Static(value) => {
                options.insert(name, value);
            }
            AnalyzedValue::Dynamic => return None,
        }
    }

    Some(())
}

fn analyze_initializer(name: &str, init: Option<&Expression<'_>>) -> AnalyzedValue {
    let Some(init) = init else {
        return AnalyzedValue::Dynamic;
    };

    if name == "load"
        && matches!(
            init,
            Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_)
        )
    {
        return AnalyzedValue::Static(Value::Null);
    }

    literal_value(init)
        .map(AnalyzedValue::Static)
        .unwrap_or(AnalyzedValue::Dynamic)
}

fn literal_value(expression: &Expression<'_>) -> Option<Value> {
    match expression {
        Expression::ArrayExpression(array) => Some(Value::Array(
            array
                .elements
                .iter()
                .map(|element| literal_value(element.as_expression()?))
                .collect::<Option<Vec<_>>>()?,
        )),
        Expression::BooleanLiteral(value) => Some(Value::Bool(value.value)),
        Expression::NullLiteral(_) => Some(Value::Null),
        Expression::NumericLiteral(value) => Number::from_f64(value.value).map(Value::Number),
        Expression::ObjectExpression(object) => Some(Value::Object({
            let mut entries = serde_json::Map::new();
            for property in &object.properties {
                match property {
                    ObjectPropertyKind::ObjectProperty(property) => {
                        if property.computed || property.method {
                            return None;
                        }

                        let key = property_key_name(&property.key)?;
                        entries.insert(key, literal_value(&property.value)?);
                    }
                    ObjectPropertyKind::SpreadProperty(property) => {
                        let Value::Object(spread_entries) = literal_value(&property.argument)?
                        else {
                            return None;
                        };
                        entries.extend(spread_entries);
                    }
                }
            }
            entries
        })),
        Expression::StringLiteral(value) => Some(Value::String(value.value.to_string())),
        Expression::TemplateLiteral(value)
            if value.expressions.is_empty() && value.quasis.len() == 1 =>
        {
            Some(Value::String(
                value.quasis[0].value.cooked.as_ref()?.to_string(),
            ))
        }
        Expression::TSAsExpression(expression) => literal_value(&expression.expression),
        Expression::TSSatisfiesExpression(expression) => literal_value(&expression.expression),
        Expression::TSNonNullExpression(expression) => literal_value(&expression.expression),
        Expression::TSInstantiationExpression(expression) => literal_value(&expression.expression),
        _ => None,
    }
}

fn binding_name(pattern: &BindingPattern<'_>) -> Option<String> {
    match pattern {
        BindingPattern::BindingIdentifier(identifier) => Some(identifier.name.as_str().to_string()),
        BindingPattern::AssignmentPattern(pattern) => binding_name(&pattern.left),
        _ => None,
    }
}

fn module_export_name(name: &ModuleExportName<'_>) -> Option<String> {
    match name {
        ModuleExportName::IdentifierName(name) => Some(name.name.as_str().to_string()),
        ModuleExportName::IdentifierReference(name) => Some(name.name.as_str().to_string()),
        ModuleExportName::StringLiteral(name) => Some(name.value.as_str().to_string()),
    }
}

fn property_key_name(key: &PropertyKey<'_>) -> Option<String> {
    match key {
        PropertyKey::StaticIdentifier(identifier) => Some(identifier.name.to_string()),
        PropertyKey::Identifier(identifier) => Some(identifier.name.to_string()),
        PropertyKey::StringLiteral(literal) => Some(literal.value.to_string()),
        PropertyKey::TemplateLiteral(literal)
            if literal.expressions.is_empty() && literal.quasis.len() == 1 =>
        {
            Some(literal.quasis[0].value.cooked.as_ref()?.to_string())
        }
        _ => None,
    }
}

fn is_valid_page_option(name: &str) -> bool {
    VALID_PAGE_OPTIONS.contains(&name)
}
