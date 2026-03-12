use crate::api::modern::{RawField, estree_node_field, estree_node_field_str, estree_node_type};
use crate::ast::modern::{EstreeNode, EstreeValue, Expression};

pub(crate) trait Render {
    fn render(&self) -> Option<String>;
}

pub(crate) fn render<T: Render + ?Sized>(value: &T) -> Option<String> {
    value.render()
}

impl Render for Expression {
    fn render(&self) -> Option<String> {
        let mut rendered = Renderer::new().expr(&self.0)?;
        for _ in 0..self.parens() {
            rendered = format!("({rendered})");
        }
        Some(rendered)
    }
}

impl Render for EstreeNode {
    fn render(&self) -> Option<String> {
        Renderer::new().expr(self)
    }
}

impl Render for EstreeValue {
    fn render(&self) -> Option<String> {
        match self {
            EstreeValue::Object(node) => node.render(),
            EstreeValue::String(value) => Some(render_string(value)),
            EstreeValue::Int(value) => Some(value.to_string()),
            EstreeValue::UInt(value) => Some(value.to_string()),
            EstreeValue::Number(value) => Some(value.to_string()),
            EstreeValue::Bool(value) => Some(value.to_string()),
            EstreeValue::Null => Some(String::from("null")),
            EstreeValue::Array(_) => None,
        }
    }
}

struct Renderer;

impl Renderer {
    const fn new() -> Self {
        Self
    }

    fn expr(&self, node: &EstreeNode) -> Option<String> {
        self.expr_with(node, Prec::Lowest)
    }

    fn expr_with(&self, node: &EstreeNode, parent: Prec) -> Option<String> {
        let rendered = match estree_node_type(node)? {
            "Identifier" => self.identifier(node)?,
            "PrivateIdentifier" => format!("#{}", field_str(node, "name")?),
            "Literal" => self.literal(node)?,
            "ThisExpression" => String::from("this"),
            "Super" => String::from("super"),
            "MetaProperty" => {
                let meta = field_object(node, "meta")?;
                let property = field_object(node, "property")?;
                format!("{}.{}", self.expr(meta)?, self.expr(property)?)
            }
            "ArrayExpression" | "ArrayPattern" => self.array(node)?,
            "ObjectExpression" | "ObjectPattern" => self.object(node)?,
            "Property" => self.property(node)?,
            "SpreadElement" => format!("...{}", self.expr(field_object(node, "argument")?)?),
            "RestElement" => {
                let mut out = format!("...{}", self.expr(field_object(node, "argument")?)?);
                if let Some(type_annotation) = field_object(node, "typeAnnotation") {
                    out.push_str(&self.type_annotation(type_annotation)?);
                }
                out
            }
            "AssignmentPattern" => format!(
                "{} = {}",
                self.expr_with(field_object(node, "left")?, Prec::Assign)?,
                self.expr_with(field_object(node, "right")?, Prec::Assign)?
            ),
            "MemberExpression" => self.member(node)?,
            "ChainExpression" => self.expr(field_object(node, "expression")?)?,
            "CallExpression" => self.call(node)?,
            "NewExpression" => self.new_expr(node)?,
            "YieldExpression" => self.yield_expression(node)?,
            "AwaitExpression" => format!(
                "await {}",
                self.expr_with(field_object(node, "argument")?, Prec::Unary)?
            ),
            "UnaryExpression" => self.unary(node)?,
            "UpdateExpression" => self.update(node)?,
            "BinaryExpression" => self.binary(node)?,
            "LogicalExpression" => self.logical(node)?,
            "ConditionalExpression" => self.conditional(node)?,
            "AssignmentExpression" => self.assignment(node)?,
            "SequenceExpression" => self.sequence(node)?,
            "ArrowFunctionExpression" => self.arrow(node)?,
            "FunctionExpression" => self.function(node)?,
            "TemplateLiteral" => self.template(node)?,
            "TaggedTemplateExpression" => format!(
                "{}{}",
                self.expr_with(field_object(node, "tag")?, Prec::Call)?,
                self.template(field_object(node, "quasi")?)?
            ),
            "ParenthesizedExpression" => {
                format!("({})", self.expr(field_object(node, "expression")?)?)
            }
            "TSAsExpression" => format!(
                "{} as {}",
                self.expr_with(field_object(node, "expression")?, Prec::Relational)?,
                self.ts(field_object(node, "typeAnnotation")?)?
            ),
            "TSSatisfiesExpression" => format!(
                "{} satisfies {}",
                self.expr_with(field_object(node, "expression")?, Prec::Relational)?,
                self.ts(field_object(node, "typeAnnotation")?)?
            ),
            "TSNonNullExpression" => format!(
                "{}!",
                self.expr_with(field_object(node, "expression")?, Prec::Member)?
            ),
            "TSTypeAssertion" => format!(
                "<{}>{}",
                self.ts(field_object(node, "typeAnnotation")?)?,
                self.expr_with(field_object(node, "expression")?, Prec::Unary)?
            ),
            "TSInstantiationExpression" => {
                let mut out = self.expr_with(field_object(node, "expression")?, Prec::Call)?;
                if let Some(params) = field_object(node, "typeParameters") {
                    out.push_str(&self.ts_type_params(params)?);
                }
                out
            }
            kind if kind.starts_with("TS") => self.ts(node)?,
            "Program" => self.program(node)?,
            "ImportDeclaration" => self.import_declaration(node)?,
            "ImportSpecifier" => self.import_specifier(node)?,
            "ImportDefaultSpecifier" => self.expr(field_object(node, "local")?)?,
            "ImportNamespaceSpecifier" => {
                format!("* as {}", self.expr(field_object(node, "local")?)?)
            }
            "ExportNamedDeclaration" => self.export_named_declaration(node)?,
            "ExportSpecifier" => self.export_specifier(node)?,
            "ExportDefaultDeclaration" => self.export_default_declaration(node)?,
            "ExportAllDeclaration" => self.export_all_declaration(node)?,
            "ExpressionStatement" => format!("{};", self.expr(field_object(node, "expression")?)?),
            "BlockStatement" => self.block(node)?,
            "EmptyStatement" => String::from(";"),
            "ReturnStatement" => {
                let mut out = String::from("return");
                if let Some(argument) = field_object(node, "argument") {
                    out.push(' ');
                    out.push_str(&self.expr(argument)?);
                }
                out.push(';');
                out
            }
            "IfStatement" => self.if_statement(node)?,
            "LabeledStatement" => self.labeled_statement(node)?,
            "ThrowStatement" => {
                format!("throw {};", self.expr(field_object(node, "argument")?)?)
            }
            "BreakStatement" => self.break_statement(node, "break")?,
            "ContinueStatement" => self.break_statement(node, "continue")?,
            "WhileStatement" => format!(
                "while ({}) {}",
                self.expr(field_object(node, "test")?)?,
                self.expr(field_object(node, "body")?)?
            ),
            "DoWhileStatement" => format!(
                "do {} while ({});",
                self.expr(field_object(node, "body")?)?,
                self.expr(field_object(node, "test")?)?
            ),
            "ForStatement" => self.for_statement(node)?,
            "ForInStatement" => self.for_each_statement(node, "in")?,
            "ForOfStatement" => self.for_each_statement(node, "of")?,
            "TryStatement" => self.try_statement(node)?,
            "CatchClause" => self.catch_clause(node)?,
            "SwitchStatement" => self.switch_statement(node)?,
            "SwitchCase" => self.switch_case(node)?,
            "DebuggerStatement" => String::from("debugger;"),
            "VariableDeclaration" => self.variable_declaration(node)?,
            "VariableDeclarator" => self.variable_declarator(node)?,
            "FunctionDeclaration" => self.function(node)?,
            "ClassDeclaration" | "ClassExpression" => self.class(node)?,
            "ClassBody" => self.class_body(node)?,
            "PropertyDefinition" => self.property_definition(node, false)?,
            "AccessorProperty" => self.property_definition(node, true)?,
            "StaticBlock" => self.static_block(node)?,
            "MethodDefinition" => self.method_definition(node)?,
            _ => return None,
        };

        let current = self.prec(node);
        Some(if current < parent {
            format!("({rendered})")
        } else {
            rendered
        })
    }

    fn identifier(&self, node: &EstreeNode) -> Option<String> {
        let mut out = estree_node_field_str(node, RawField::Name)?.to_string();
        if let Some(optional) = field_bool(node, "optional")
            && optional
        {
            out.push('?');
        }
        if let Some(type_annotation) = field_object(node, "typeAnnotation") {
            out.push_str(&self.type_annotation(type_annotation)?);
        }
        Some(out)
    }

    fn literal(&self, node: &EstreeNode) -> Option<String> {
        if let Some(raw) = estree_node_field_str(node, RawField::Raw)
            && !raw.is_empty()
        {
            return Some(raw.to_string());
        }
        match estree_node_field(node, RawField::Value)? {
            EstreeValue::String(value) => Some(render_string(value)),
            EstreeValue::Int(value) => Some(value.to_string()),
            EstreeValue::UInt(value) => Some(value.to_string()),
            EstreeValue::Number(value) => Some(value.to_string()),
            EstreeValue::Bool(value) => Some(value.to_string()),
            EstreeValue::Null => Some(String::from("null")),
            EstreeValue::Object(_) | EstreeValue::Array(_) => None,
        }
    }

    fn array(&self, node: &EstreeNode) -> Option<String> {
        let mut out = String::from("[");
        if let Some(elements) = field_array(node, "elements") {
            for (index, element) in elements.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                match element {
                    EstreeValue::Null => {}
                    _ => out.push_str(&self.value(element)?),
                }
            }
        }
        out.push(']');
        if let Some(type_annotation) = field_object(node, "typeAnnotation") {
            out.push_str(&self.type_annotation(type_annotation)?);
        }
        Some(out)
    }

    fn object(&self, node: &EstreeNode) -> Option<String> {
        let properties = field_array(node, "properties").unwrap_or(&[]);
        let mut out = String::from("{");
        if !properties.is_empty() {
            out.push(' ');
            for (index, property) in properties.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(&self.value(property)?);
            }
            out.push(' ');
        }
        out.push('}');
        if let Some(type_annotation) = field_object(node, "typeAnnotation") {
            out.push_str(&self.type_annotation(type_annotation)?);
        }
        Some(out)
    }

    fn property(&self, node: &EstreeNode) -> Option<String> {
        let shorthand = field_bool(node, "shorthand").unwrap_or(false);
        let computed = field_bool(node, "computed").unwrap_or(false);
        let key = field_object(node, "key")?;
        let value = field_object(node, "value")?;

        if shorthand {
            return self.expr(value);
        }

        let key = if computed {
            format!("[{}]", self.expr(key)?)
        } else {
            self.property_key(key)?
        };

        let kind = field_str(node, "kind").unwrap_or("init");
        if kind == "get" || kind == "set" {
            return Some(format!("{kind} {key}{}", self.function_tail(value)?));
        }

        if estree_node_type(value) == Some("FunctionExpression")
            && !field_bool(value, "typeAnnotation").unwrap_or(false)
        {
            let prefix = if field_bool(value, "async").unwrap_or(false) {
                "async "
            } else {
                ""
            };
            let generator = if field_bool(value, "generator").unwrap_or(false) {
                "*"
            } else {
                ""
            };
            return Some(format!(
                "{prefix}{generator}{key}{}",
                self.function_tail(value)?
            ));
        }

        Some(format!("{key}: {}", self.expr(value)?))
    }

    fn member(&self, node: &EstreeNode) -> Option<String> {
        let object = self.expr_with(field_object(node, "object")?, Prec::Member)?;
        let property = field_object(node, "property")?;
        let computed = field_bool(node, "computed").unwrap_or(false);
        let optional = field_bool(node, "optional").unwrap_or(false);

        Some(if computed {
            let glue = if optional { "?.[" } else { "[" };
            format!("{object}{glue}{}]", self.expr(property)?)
        } else {
            let glue = if optional { "?." } else { "." };
            format!("{object}{glue}{}", self.property_key(property)?)
        })
    }

    fn call(&self, node: &EstreeNode) -> Option<String> {
        let callee = self.expr_with(field_object(node, "callee")?, Prec::Call)?;
        let args = self.join_array(node, "arguments")?;
        let optional = field_bool(node, "optional").unwrap_or(false);
        let call = if optional { "?.(" } else { "(" };
        Some(format!("{callee}{call}{args})"))
    }

    fn new_expr(&self, node: &EstreeNode) -> Option<String> {
        let callee = self.expr_with(field_object(node, "callee")?, Prec::New)?;
        let args = self.join_array(node, "arguments")?;
        Some(format!("new {callee}({args})"))
    }

    fn yield_expression(&self, node: &EstreeNode) -> Option<String> {
        let mut out = String::from("yield");
        if field_bool(node, "delegate").unwrap_or(false) {
            out.push('*');
        }
        if let Some(argument) = field_object(node, "argument") {
            out.push(' ');
            out.push_str(&self.expr(argument)?);
        }
        Some(out)
    }

    fn unary(&self, node: &EstreeNode) -> Option<String> {
        let operator = field_str(node, "operator")?;
        let argument = self.expr_with(field_object(node, "argument")?, Prec::Unary)?;
        let spaced = matches!(operator, "delete" | "void" | "typeof" | "await");
        Some(if spaced {
            format!("{operator} {argument}")
        } else {
            format!("{operator}{argument}")
        })
    }

    fn update(&self, node: &EstreeNode) -> Option<String> {
        let operator = field_str(node, "operator")?;
        let argument = self.expr_with(field_object(node, "argument")?, Prec::Unary)?;
        Some(if field_bool(node, "prefix").unwrap_or(false) {
            format!("{operator}{argument}")
        } else {
            format!("{argument}{operator}")
        })
    }

    fn binary(&self, node: &EstreeNode) -> Option<String> {
        let operator = field_str(node, "operator")?;
        let current = Prec::from_operator(operator);
        let left = self.expr_with(field_object(node, "left")?, current)?;
        let right = self.expr_with(field_object(node, "right")?, current.next())?;
        Some(format!("{left} {operator} {right}"))
    }

    fn logical(&self, node: &EstreeNode) -> Option<String> {
        let operator = field_str(node, "operator")?;
        let current = Prec::from_operator(operator);
        let left = self.expr_with(field_object(node, "left")?, current)?;
        let right = self.expr_with(field_object(node, "right")?, current.next())?;
        Some(format!("{left} {operator} {right}"))
    }

    fn conditional(&self, node: &EstreeNode) -> Option<String> {
        Some(format!(
            "{} ? {} : {}",
            self.expr_with(field_object(node, "test")?, Prec::Conditional)?,
            self.expr_with(field_object(node, "consequent")?, Prec::Assign)?,
            self.expr_with(field_object(node, "alternate")?, Prec::Assign)?
        ))
    }

    fn assignment(&self, node: &EstreeNode) -> Option<String> {
        let operator = field_str(node, "operator")?;
        Some(format!(
            "{} {operator} {}",
            self.expr_with(field_object(node, "left")?, Prec::Assign.next())?,
            self.expr_with(field_object(node, "right")?, Prec::Assign)?
        ))
    }

    fn sequence(&self, node: &EstreeNode) -> Option<String> {
        self.join_array(node, "expressions")
    }

    fn arrow(&self, node: &EstreeNode) -> Option<String> {
        let mut out = String::new();
        if field_bool(node, "async").unwrap_or(false) {
            out.push_str("async ");
        }

        let params = self.params(node)?;
        if params.len() == 1 && is_simple_param(&params[0]) {
            out.push_str(&params[0]);
        } else {
            out.push('(');
            out.push_str(&params.join(", "));
            out.push(')');
        }

        out.push_str(" => ");
        let body = field_object(node, "body")?;
        out.push_str(&match estree_node_type(body) {
            Some("BlockStatement") => self.block(body)?,
            _ => self.expr(body)?,
        });
        Some(out)
    }

    fn function(&self, node: &EstreeNode) -> Option<String> {
        let mut out = String::new();
        if field_bool(node, "async").unwrap_or(false) {
            out.push_str("async ");
        }
        out.push_str("function");
        if field_bool(node, "generator").unwrap_or(false) {
            out.push('*');
        }
        if let Some(id) = field_object(node, "id") {
            out.push(' ');
            out.push_str(&self.expr(id)?);
        }
        out.push_str(&self.function_tail(node)?);
        Some(out)
    }

    fn function_tail(&self, node: &EstreeNode) -> Option<String> {
        let params = self.params(node)?.join(", ");
        let mut out = String::new();
        if let Some(type_params) = field_object(node, "typeParameters") {
            out.push_str(&self.ts_type_params(type_params)?);
        }
        out.push('(');
        out.push_str(&params);
        out.push(')');
        if let Some(return_type) = field_object(node, "returnType") {
            out.push_str(&self.type_annotation(return_type)?);
        }
        out.push(' ');
        out.push_str(&self.block(field_object(node, "body")?)?);
        Some(out)
    }

    fn template(&self, node: &EstreeNode) -> Option<String> {
        let quasis = field_array(node, "quasis")?;
        let expressions = field_array(node, "expressions").unwrap_or(&[]);
        let mut out = String::from("`");
        for index in 0..quasis.len() {
            let quasi = value_object(quasis.get(index)?)?;
            out.push_str(&self.template_part(quasi)?);
            if let Some(expression) = expressions.get(index) {
                out.push_str("${");
                out.push_str(&self.value(expression)?);
                out.push('}');
            }
        }
        out.push('`');
        Some(out)
    }

    fn template_part(&self, node: &EstreeNode) -> Option<String> {
        if let Some(value) = field_object(node, "value") {
            if let Some(raw) = field_str(value, "raw") {
                return Some(raw.to_string());
            }
            if let Some(cooked) = field_str(value, "cooked") {
                return Some(escape_template(cooked));
            }
        }
        Some(String::new())
    }

    fn block(&self, node: &EstreeNode) -> Option<String> {
        let body = field_array(node, "body").unwrap_or(&[]);
        if body.is_empty() {
            return Some(String::from("{}"));
        }
        if body.len() == 1 {
            return Some(format!("{{ {} }}", self.value(body.first()?)?));
        }
        let mut out = String::from("{\n");
        for statement in body {
            out.push('\t');
            out.push_str(&self.value(statement)?);
            out.push('\n');
        }
        out.push('}');
        Some(out)
    }

    fn if_statement(&self, node: &EstreeNode) -> Option<String> {
        let test = self.expr(field_object(node, "test")?)?;
        let consequent = self.expr(field_object(node, "consequent")?)?;
        let mut out = format!("if ({test}) {consequent}");
        if let Some(alternate) = field_object(node, "alternate") {
            out.push_str(" else ");
            out.push_str(&self.expr(alternate)?);
        }
        Some(out)
    }

    fn labeled_statement(&self, node: &EstreeNode) -> Option<String> {
        Some(format!(
            "{}: {}",
            self.expr(field_object(node, "label")?)?,
            self.expr(field_object(node, "body")?)?
        ))
    }

    fn break_statement(&self, node: &EstreeNode, keyword: &str) -> Option<String> {
        let mut out = String::from(keyword);
        if let Some(label) = field_object(node, "label") {
            out.push(' ');
            out.push_str(&self.expr(label)?);
        }
        out.push(';');
        Some(out)
    }

    fn for_statement(&self, node: &EstreeNode) -> Option<String> {
        let init = field_object(node, "init")
            .and_then(|value| self.expr(value))
            .unwrap_or_default();
        let test = field_object(node, "test")
            .and_then(|value| self.expr(value))
            .unwrap_or_default();
        let update = field_object(node, "update")
            .and_then(|value| self.expr(value))
            .unwrap_or_default();
        let body = self.expr(field_object(node, "body")?)?;
        Some(format!("for ({init}; {test}; {update}) {body}"))
    }

    fn for_each_statement(&self, node: &EstreeNode, operator: &str) -> Option<String> {
        let left = self.expr(field_object(node, "left")?)?;
        let right = self.expr(field_object(node, "right")?)?;
        let body = self.expr(field_object(node, "body")?)?;
        Some(format!("for ({left} {operator} {right}) {body}"))
    }

    fn try_statement(&self, node: &EstreeNode) -> Option<String> {
        let mut out = format!("try {}", self.expr(field_object(node, "block")?)?);
        if let Some(handler) = field_object(node, "handler") {
            out.push(' ');
            out.push_str(&self.expr(handler)?);
        }
        if let Some(finalizer) = field_object(node, "finalizer") {
            out.push_str(" finally ");
            out.push_str(&self.expr(finalizer)?);
        }
        Some(out)
    }

    fn catch_clause(&self, node: &EstreeNode) -> Option<String> {
        let mut out = String::from("catch");
        if let Some(param) = field_object(node, "param") {
            out.push_str(" (");
            out.push_str(&self.expr(param)?);
            out.push(')');
        }
        out.push(' ');
        out.push_str(&self.expr(field_object(node, "body")?)?);
        Some(out)
    }

    fn switch_statement(&self, node: &EstreeNode) -> Option<String> {
        let discriminant = self.expr(field_object(node, "discriminant")?)?;
        let cases = field_array(node, "cases").unwrap_or(&[]);
        let rendered_cases = cases
            .iter()
            .map(|value| self.value(value))
            .collect::<Option<Vec<_>>>()?;
        Some(if rendered_cases.is_empty() {
            format!("switch ({discriminant}) {{}}")
        } else {
            format!("switch ({discriminant}) {{ {} }}", rendered_cases.join(" "))
        })
    }

    fn switch_case(&self, node: &EstreeNode) -> Option<String> {
        let head = if let Some(test) = field_object(node, "test") {
            format!("case {}:", self.expr(test)?)
        } else {
            String::from("default:")
        };
        let consequent = field_array(node, "consequent").unwrap_or(&[]);
        if consequent.is_empty() {
            return Some(head);
        }
        let rendered = consequent
            .iter()
            .map(|value| self.value(value))
            .collect::<Option<Vec<_>>>()?;
        Some(format!("{head} {}", rendered.join(" ")))
    }

    fn variable_declaration(&self, node: &EstreeNode) -> Option<String> {
        let kind = field_str(node, "kind")?;
        let declarations = self.join_array(node, "declarations")?;
        Some(format!("{kind} {declarations};"))
    }

    fn variable_declarator(&self, node: &EstreeNode) -> Option<String> {
        let mut out = self.expr(field_object(node, "id")?)?;
        if let Some(init) = field_object(node, "init") {
            out.push_str(" = ");
            out.push_str(&self.expr(init)?);
        }
        Some(out)
    }

    fn class(&self, node: &EstreeNode) -> Option<String> {
        let mut out = String::new();
        if field_bool(node, "abstract").unwrap_or(false)
            || estree_node_type(node) == Some("TSAbstractMethodDefinition")
        {
            out.push_str("abstract ");
        }
        out.push_str("class");
        if let Some(id) = field_object(node, "id") {
            out.push(' ');
            out.push_str(&self.expr(id)?);
        }
        if let Some(type_params) = field_object(node, "typeParameters") {
            out.push_str(&self.ts_type_params(type_params)?);
        }
        if let Some(super_class) = field_object(node, "superClass") {
            out.push_str(" extends ");
            out.push_str(&self.expr_with(super_class, Prec::Call)?);
        }
        if let Some(implements) = field_array(node, "implements")
            && !implements.is_empty()
        {
            let implements = implements
                .iter()
                .map(|value| self.value(value))
                .collect::<Option<Vec<_>>>()?;
            out.push_str(" implements ");
            out.push_str(&implements.join(", "));
        }
        out.push(' ');
        out.push_str(&self.class_body(field_object(node, "body")?)?);
        Some(out)
    }

    fn class_body(&self, node: &EstreeNode) -> Option<String> {
        let body = field_array(node, "body").unwrap_or(&[]);
        if body.is_empty() {
            return Some(String::from("{}"));
        }

        let members = body
            .iter()
            .map(|value| self.value(value))
            .collect::<Option<Vec<_>>>()?;
        Some(format!("{{ {} }}", members.join(" ")))
    }

    fn property_definition(&self, node: &EstreeNode, accessor: bool) -> Option<String> {
        let mut out = String::new();
        if field_bool(node, "declare").unwrap_or(false) {
            out.push_str("declare ");
        }
        if field_bool(node, "static").unwrap_or(false) {
            out.push_str("static ");
        }
        if field_bool(node, "override").unwrap_or(false) {
            out.push_str("override ");
        }
        if let Some(accessibility) = field_str(node, "accessibility") {
            out.push_str(accessibility);
            out.push(' ');
        }
        if field_bool(node, "readonly").unwrap_or(false) {
            out.push_str("readonly ");
        }
        if accessor {
            out.push_str("accessor ");
        }

        let key = field_object(node, "key")?;
        if field_bool(node, "computed").unwrap_or(false) {
            out.push('[');
            out.push_str(&self.expr(key)?);
            out.push(']');
        } else {
            out.push_str(&self.property_key(key)?);
        }

        if field_bool(node, "optional").unwrap_or(false) {
            out.push('?');
        }
        if field_bool(node, "definite").unwrap_or(false) {
            out.push('!');
        }
        if let Some(type_annotation) = field_object(node, "typeAnnotation") {
            out.push_str(&self.type_annotation(type_annotation)?);
        }
        if let Some(value) = field_object(node, "value") {
            out.push_str(" = ");
            out.push_str(&self.expr(value)?);
        }
        out.push(';');
        Some(out)
    }

    fn static_block(&self, node: &EstreeNode) -> Option<String> {
        let body = field_array(node, "body").unwrap_or(&[]);
        if body.is_empty() {
            return Some(String::from("static {}"));
        }

        let mut out = String::from("static {\n");
        for statement in body {
            out.push('\t');
            out.push_str(&self.value(statement)?);
            out.push('\n');
        }
        out.push('}');
        Some(out)
    }

    fn method_definition(&self, node: &EstreeNode) -> Option<String> {
        let mut out = String::new();
        if field_bool(node, "static").unwrap_or(false) {
            out.push_str("static ");
        }
        if let Some(accessibility) = field_str(node, "accessibility") {
            out.push_str(accessibility);
            out.push(' ');
        }
        if field_bool(node, "override").unwrap_or(false) {
            out.push_str("override ");
        }
        if field_bool(node, "abstract").unwrap_or(false)
            || estree_node_type(node) == Some("TSAbstractMethodDefinition")
        {
            out.push_str("abstract ");
        }

        let kind = field_str(node, "kind").unwrap_or("method");
        if kind == "get" || kind == "set" {
            out.push_str(kind);
            out.push(' ');
        }

        let value = field_object(node, "value")?;
        if kind == "method" && field_bool(value, "async").unwrap_or(false) {
            out.push_str("async ");
        }
        if kind == "method" && field_bool(value, "generator").unwrap_or(false) {
            out.push('*');
        }

        let key = field_object(node, "key")?;
        if field_bool(node, "computed").unwrap_or(false) {
            out.push('[');
            out.push_str(&self.expr(key)?);
            out.push(']');
        } else {
            out.push_str(&self.property_key(key)?);
        }

        if let Some(type_params) = field_object(value, "typeParameters") {
            out.push_str(&self.ts_type_params(type_params)?);
        }
        out.push('(');
        out.push_str(&self.params(value)?.join(", "));
        out.push(')');
        if let Some(return_type) = field_object(value, "returnType") {
            out.push_str(&self.type_annotation(return_type)?);
        }
        if let Some(body) = field_object(value, "body") {
            out.push(' ');
            out.push_str(&self.block(body)?);
        } else {
            out.push(';');
        }
        Some(out)
    }

    fn program(&self, node: &EstreeNode) -> Option<String> {
        let body = field_array(node, "body").unwrap_or(&[]);
        let mut out = String::new();
        for (index, statement) in body.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            out.push_str(&self.value(statement)?);
        }
        Some(out)
    }

    fn import_declaration(&self, node: &EstreeNode) -> Option<String> {
        let mut out = String::from("import");
        let specifiers = field_array(node, "specifiers").unwrap_or(&[]);

        if !specifiers.is_empty() {
            out.push(' ');
            out.push_str(&self.import_specifiers(specifiers)?);
            out.push_str(" from ");
        } else {
            out.push(' ');
        }

        out.push_str(&self.expr(field_object(node, "source")?)?);
        out.push(';');
        Some(out)
    }

    fn import_specifiers(&self, specifiers: &[EstreeValue]) -> Option<String> {
        let mut default = None;
        let mut namespace = None;
        let mut named = Vec::new();

        for specifier in specifiers {
            let node = value_object(specifier)?;
            match estree_node_type(node)? {
                "ImportDefaultSpecifier" => {
                    default = Some(self.expr(field_object(node, "local")?)?)
                }
                "ImportNamespaceSpecifier" => {
                    namespace = Some(format!("* as {}", self.expr(field_object(node, "local")?)?));
                }
                "ImportSpecifier" => named.push(self.import_specifier(node)?),
                _ => return None,
            }
        }

        let mut parts = Vec::new();
        if let Some(default) = default {
            parts.push(default);
        }
        if let Some(namespace) = namespace {
            parts.push(namespace);
        }
        if !named.is_empty() {
            parts.push(format!("{{ {} }}", named.join(", ")));
        }
        Some(parts.join(", "))
    }

    fn import_specifier(&self, node: &EstreeNode) -> Option<String> {
        let imported = self.property_key(field_object(node, "imported")?)?;
        let local = self.expr(field_object(node, "local")?)?;
        Some(if imported == local {
            imported
        } else {
            format!("{imported} as {local}")
        })
    }

    fn export_named_declaration(&self, node: &EstreeNode) -> Option<String> {
        let mut out = String::from("export ");
        if let Some(declaration) = field_object(node, "declaration") {
            out.push_str(&self.expr(declaration)?);
            return Some(out);
        }

        let specifiers = field_array(node, "specifiers").unwrap_or(&[]);
        out.push('{');
        if !specifiers.is_empty() {
            out.push(' ');
            out.push_str(
                &specifiers
                    .iter()
                    .map(|value| self.value(value))
                    .collect::<Option<Vec<_>>>()?
                    .join(", "),
            );
            out.push(' ');
        }
        out.push('}');
        if let Some(source) = field_object(node, "source") {
            out.push_str(" from ");
            out.push_str(&self.expr(source)?);
        }
        out.push(';');
        Some(out)
    }

    fn export_specifier(&self, node: &EstreeNode) -> Option<String> {
        let local = self.property_key(field_object(node, "local")?)?;
        let exported = self.property_key(field_object(node, "exported")?)?;
        Some(if local == exported {
            local
        } else {
            format!("{local} as {exported}")
        })
    }

    fn export_default_declaration(&self, node: &EstreeNode) -> Option<String> {
        let declaration = field_object(node, "declaration")?;
        let rendered = self.expr(declaration)?;
        let needs_semicolon = !matches!(
            estree_node_type(declaration),
            Some("FunctionDeclaration" | "ClassDeclaration")
        );
        Some(if needs_semicolon {
            format!("export default {rendered};")
        } else {
            format!("export default {rendered}")
        })
    }

    fn export_all_declaration(&self, node: &EstreeNode) -> Option<String> {
        let mut out = String::from("export *");
        if let Some(exported) = field_object(node, "exported") {
            out.push_str(" as ");
            out.push_str(&self.property_key(exported)?);
        }
        out.push_str(" from ");
        out.push_str(&self.expr(field_object(node, "source")?)?);
        out.push(';');
        Some(out)
    }

    fn params(&self, node: &EstreeNode) -> Option<Vec<String>> {
        field_array(node, "params")
            .unwrap_or(&[])
            .iter()
            .map(|value| self.value(value))
            .collect::<Option<Vec<_>>>()
    }

    fn value(&self, value: &EstreeValue) -> Option<String> {
        match value {
            EstreeValue::Object(node) => self.expr(node),
            EstreeValue::String(value) => Some(render_string(value)),
            EstreeValue::Int(value) => Some(value.to_string()),
            EstreeValue::UInt(value) => Some(value.to_string()),
            EstreeValue::Number(value) => Some(value.to_string()),
            EstreeValue::Bool(value) => Some(value.to_string()),
            EstreeValue::Null => Some(String::from("null")),
            EstreeValue::Array(_) => None,
        }
    }

    fn property_key(&self, node: &EstreeNode) -> Option<String> {
        match estree_node_type(node) {
            Some("Identifier") => {
                estree_node_field_str(node, RawField::Name).map(ToString::to_string)
            }
            Some("PrivateIdentifier") => field_str(node, "name").map(|name| format!("#{name}")),
            _ => self.expr(node),
        }
    }

    fn type_annotation(&self, node: &EstreeNode) -> Option<String> {
        Some(format!(
            ": {}",
            self.ts(field_object(node, "typeAnnotation")?)?
        ))
    }

    fn ts_type_params(&self, node: &EstreeNode) -> Option<String> {
        let params = field_array(node, "params")?;
        let joined = params
            .iter()
            .map(|value| self.value(value))
            .collect::<Option<Vec<_>>>()?
            .join(", ");
        Some(format!("<{joined}>"))
    }

    fn ts(&self, node: &EstreeNode) -> Option<String> {
        match estree_node_type(node)? {
            "TSTypeAnnotation" => self.type_annotation(node),
            "TSInterfaceDeclaration" => self.ts_interface_declaration(node),
            "TSInterfaceBody" => self.ts_interface_body(node),
            "TSTypeAliasDeclaration" => self.ts_type_alias_declaration(node),
            "TSDeclareFunction" => self.ts_declare_function(node),
            "TSModuleDeclaration" => self.ts_module_declaration(node),
            "TSModuleBlock" => self.ts_module_block(node),
            "TSAbstractMethodDefinition" => self.method_definition(node),
            "TSClassImplements" => {
                let mut out = self.expr(field_object(node, "expression")?)?;
                if let Some(params) = field_object(node, "typeParameters") {
                    out.push_str(&self.ts_type_params(params)?);
                } else if let Some(args) = field_object(node, "typeArguments") {
                    out.push_str(&self.ts_type_params(args)?);
                }
                Some(out)
            }
            "TSTypeReference" => {
                let mut out = self.ts(field_object(node, "typeName")?)?;
                if let Some(params) = field_object(node, "typeParameters") {
                    out.push_str(&self.ts_type_params(params)?);
                }
                Some(out)
            }
            "TSExpressionWithTypeArguments" => {
                let mut out = self.expr(field_object(node, "expression")?)?;
                if let Some(params) = field_object(node, "typeParameters") {
                    out.push_str(&self.ts_type_params(params)?);
                } else if let Some(args) = field_object(node, "typeArguments") {
                    out.push_str(&self.ts_type_params(args)?);
                }
                Some(out)
            }
            "TSQualifiedName" => Some(format!(
                "{}.{}",
                self.ts(field_object(node, "left")?)?,
                self.ts(field_object(node, "right")?)?
            )),
            "TSArrayType" => Some(format!(
                "{}[]",
                self.ts(field_object(node, "elementType")?)?
            )),
            "TSTupleType" => {
                let elements = field_array(node, "elementTypes")?;
                let joined = elements
                    .iter()
                    .map(|value| self.value(value))
                    .collect::<Option<Vec<_>>>()?
                    .join(", ");
                Some(format!("[{joined}]"))
            }
            "TSUnionType" => self.join_ts(node, "types", " | "),
            "TSIntersectionType" => self.join_ts(node, "types", " & "),
            "TSLiteralType" => self.expr(field_object(node, "literal")?),
            "TSTypeParameterDeclaration" => self.ts_type_params(node),
            "TSTypeParameter" => {
                let mut out = self.expr(field_object(node, "name")?)?;
                if let Some(constraint) = field_object(node, "constraint") {
                    out.push_str(" extends ");
                    out.push_str(&self.ts(constraint)?);
                }
                if let Some(default) = field_object(node, "default") {
                    out.push_str(" = ");
                    out.push_str(&self.ts(default)?);
                }
                Some(out)
            }
            "TSParenthesizedType" => Some(format!(
                "({})",
                self.ts(field_object(node, "typeAnnotation")?)?
            )),
            "TSOptionalType" => Some(format!(
                "{}?",
                self.ts(field_object(node, "typeAnnotation")?)?
            )),
            "TSRestType" => Some(format!(
                "...{}",
                self.ts(field_object(node, "typeAnnotation")?)?
            )),
            "TSTypeLiteral" => {
                let members = field_array(node, "members").unwrap_or(&[]);
                let joined = members
                    .iter()
                    .map(|value| self.value(value))
                    .collect::<Option<Vec<_>>>()?
                    .join("; ");
                Some(if joined.is_empty() {
                    String::from("{}")
                } else {
                    format!("{{ {joined} }}")
                })
            }
            "TSPropertySignature" => {
                let mut out = String::new();
                if field_bool(node, "readonly").unwrap_or(false) {
                    out.push_str("readonly ");
                }
                let key = field_object(node, "key")?;
                if field_bool(node, "computed").unwrap_or(false) {
                    out.push('[');
                    out.push_str(&self.expr(key)?);
                    out.push(']');
                } else {
                    out.push_str(&self.property_key(key)?);
                }
                if field_bool(node, "optional").unwrap_or(false) {
                    out.push('?');
                }
                if let Some(type_annotation) = field_object(node, "typeAnnotation") {
                    out.push_str(&self.type_annotation(type_annotation)?);
                }
                Some(out)
            }
            "TSFunctionType" => {
                let params = self.params(node)?.join(", ");
                let return_type = field_object(node, "returnType")
                    .and_then(|value| self.type_annotation(value))
                    .unwrap_or_default();
                Some(format!("({params}) =>{}", return_type))
            }
            "TSAnyKeyword" => Some(String::from("any")),
            "TSUnknownKeyword" => Some(String::from("unknown")),
            "TSNeverKeyword" => Some(String::from("never")),
            "TSVoidKeyword" => Some(String::from("void")),
            "TSNullKeyword" => Some(String::from("null")),
            "TSUndefinedKeyword" => Some(String::from("undefined")),
            "TSStringKeyword" => Some(String::from("string")),
            "TSNumberKeyword" => Some(String::from("number")),
            "TSBooleanKeyword" => Some(String::from("boolean")),
            "TSObjectKeyword" => Some(String::from("object")),
            "TSBigIntKeyword" => Some(String::from("bigint")),
            "TSSymbolKeyword" => Some(String::from("symbol")),
            "TSIntrinsicKeyword" => Some(String::from("intrinsic")),
            "Identifier" | "PrivateIdentifier" => self.expr(node),
            _ => None,
        }
    }

    fn ts_interface_declaration(&self, node: &EstreeNode) -> Option<String> {
        let mut out = String::new();
        if field_bool(node, "declare").unwrap_or(false) {
            out.push_str("declare ");
        }
        out.push_str("interface ");
        out.push_str(&self.expr(field_object(node, "id")?)?);
        if let Some(type_params) = field_object(node, "typeParameters") {
            out.push_str(&self.ts_type_params(type_params)?);
        }
        if let Some(extends) = field_array(node, "extends")
            && !extends.is_empty()
        {
            let extends = extends
                .iter()
                .map(|value| self.value(value))
                .collect::<Option<Vec<_>>>()?;
            out.push_str(" extends ");
            out.push_str(&extends.join(", "));
        }
        out.push(' ');
        out.push_str(&self.ts(field_object(node, "body")?)?);
        Some(out)
    }

    fn ts_interface_body(&self, node: &EstreeNode) -> Option<String> {
        let members = field_array(node, "body").unwrap_or(&[]);
        if members.is_empty() {
            return Some(String::from("{}"));
        }
        let joined = members
            .iter()
            .map(|value| self.value(value))
            .collect::<Option<Vec<_>>>()?
            .join(" ");
        Some(format!("{{ {joined} }}"))
    }

    fn ts_type_alias_declaration(&self, node: &EstreeNode) -> Option<String> {
        let mut out = String::new();
        if field_bool(node, "declare").unwrap_or(false) {
            out.push_str("declare ");
        }
        out.push_str("type ");
        out.push_str(&self.expr(field_object(node, "id")?)?);
        if let Some(type_params) = field_object(node, "typeParameters") {
            out.push_str(&self.ts_type_params(type_params)?);
        }
        out.push_str(" = ");
        out.push_str(&self.ts(field_object(node, "typeAnnotation")?)?);
        out.push(';');
        Some(out)
    }

    fn ts_declare_function(&self, node: &EstreeNode) -> Option<String> {
        let mut out = String::from("declare function");
        if let Some(id) = field_object(node, "id") {
            out.push(' ');
            out.push_str(&self.expr(id)?);
        }
        if let Some(type_params) = field_object(node, "typeParameters") {
            out.push_str(&self.ts_type_params(type_params)?);
        }
        out.push('(');
        out.push_str(&self.params(node)?.join(", "));
        out.push(')');
        if let Some(return_type) = field_object(node, "returnType") {
            out.push_str(&self.type_annotation(return_type)?);
        }
        out.push(';');
        Some(out)
    }

    fn ts_module_declaration(&self, node: &EstreeNode) -> Option<String> {
        let mut out = String::new();
        if field_bool(node, "declare").unwrap_or(false) {
            out.push_str("declare ");
        }
        let keyword = match field_str(node, "kind") {
            Some("namespace") => "namespace",
            _ => "module",
        };
        out.push_str(keyword);
        out.push(' ');
        out.push_str(&self.value(field(node, "id")?)?);
        if let Some(body) = field_object(node, "body") {
            out.push(' ');
            out.push_str(&self.ts(body)?);
        } else {
            out.push(';');
        }
        Some(out)
    }

    fn ts_module_block(&self, node: &EstreeNode) -> Option<String> {
        let body = field_array(node, "body").unwrap_or(&[]);
        if body.is_empty() {
            return Some(String::from("{}"));
        }
        let rendered = body
            .iter()
            .map(|value| self.value(value))
            .collect::<Option<Vec<_>>>()?;
        Some(format!("{{ {} }}", rendered.join(" ")))
    }

    fn join_ts(&self, node: &EstreeNode, field: &str, separator: &str) -> Option<String> {
        let values = field_array(node, field)?;
        Some(
            values
                .iter()
                .map(|value| self.value(value))
                .collect::<Option<Vec<_>>>()?
                .join(separator),
        )
    }

    fn join_array(&self, node: &EstreeNode, field: &str) -> Option<String> {
        Some(
            field_array(node, field)
                .unwrap_or(&[])
                .iter()
                .map(|value| self.value(value))
                .collect::<Option<Vec<_>>>()?
                .join(", "),
        )
    }

    fn prec(&self, node: &EstreeNode) -> Prec {
        match estree_node_type(node) {
            Some("SequenceExpression") => Prec::Sequence,
            Some("AssignmentExpression") | Some("ArrowFunctionExpression") => Prec::Assign,
            Some("ConditionalExpression") => Prec::Conditional,
            Some("LogicalExpression") => match field_str(node, "operator") {
                Some("||" | "??") => Prec::LogicalOr,
                _ => Prec::LogicalAnd,
            },
            Some("BinaryExpression") => {
                Prec::from_operator(field_str(node, "operator").unwrap_or(""))
            }
            Some("TSAsExpression" | "TSSatisfiesExpression") => Prec::Relational,
            Some("UnaryExpression" | "AwaitExpression" | "UpdateExpression") => Prec::Unary,
            Some("CallExpression" | "NewExpression") => Prec::Call,
            Some("MemberExpression" | "ChainExpression" | "TSNonNullExpression") => Prec::Member,
            _ => Prec::Primary,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Prec {
    Lowest = 0,
    Sequence,
    Assign,
    Conditional,
    LogicalOr,
    LogicalAnd,
    Equality,
    Relational,
    Additive,
    Multiplicative,
    Exponent,
    Unary,
    New,
    Call,
    Member,
    Primary,
}

impl Prec {
    fn from_operator(operator: &str) -> Self {
        match operator {
            "||" | "??" => Self::LogicalOr,
            "&&" => Self::LogicalAnd,
            "==" | "!=" | "===" | "!==" => Self::Equality,
            "<" | "<=" | ">" | ">=" | "in" | "instanceof" => Self::Relational,
            "+" | "-" => Self::Additive,
            "*" | "/" | "%" => Self::Multiplicative,
            "**" => Self::Exponent,
            _ => Self::Primary,
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Lowest => Self::Sequence,
            Self::Sequence => Self::Assign,
            Self::Assign => Self::Conditional,
            Self::Conditional => Self::LogicalOr,
            Self::LogicalOr => Self::LogicalAnd,
            Self::LogicalAnd => Self::Equality,
            Self::Equality => Self::Relational,
            Self::Relational => Self::Additive,
            Self::Additive => Self::Multiplicative,
            Self::Multiplicative => Self::Exponent,
            Self::Exponent => Self::Unary,
            Self::Unary => Self::New,
            Self::New => Self::Call,
            Self::Call => Self::Member,
            Self::Member | Self::Primary => Self::Primary,
        }
    }
}

fn field<'a>(node: &'a EstreeNode, name: &str) -> Option<&'a EstreeValue> {
    node.fields.get(name)
}

fn field_object<'a>(node: &'a EstreeNode, name: &str) -> Option<&'a EstreeNode> {
    match field(node, name) {
        Some(EstreeValue::Object(value)) => Some(value),
        _ => None,
    }
}

fn field_array<'a>(node: &'a EstreeNode, name: &str) -> Option<&'a [EstreeValue]> {
    match field(node, name) {
        Some(EstreeValue::Array(values)) => Some(values),
        _ => None,
    }
}

fn field_str<'a>(node: &'a EstreeNode, name: &str) -> Option<&'a str> {
    match field(node, name) {
        Some(EstreeValue::String(value)) => Some(value),
        _ => None,
    }
}

fn field_bool(node: &EstreeNode, name: &str) -> Option<bool> {
    match field(node, name) {
        Some(EstreeValue::Bool(value)) => Some(*value),
        _ => None,
    }
}

fn value_object(value: &EstreeValue) -> Option<&EstreeNode> {
    match value {
        EstreeValue::Object(node) => Some(node),
        _ => None,
    }
}

fn render_string(value: &str) -> String {
    format!(
        "'{}'",
        value
            .replace('\\', "\\\\")
            .replace('\'', "\\'")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    )
}

fn escape_template(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${")
}

fn is_simple_param(value: &str) -> bool {
    !value.contains([' ', ':', '=', ',', '{', '[', '.'])
}

#[cfg(test)]
mod tests {
    use super::render;
    use crate::api::modern::{RawField, estree_node_field_array};
    use crate::ast::modern::Node;
    use crate::compiler::phases::parse::parse_component_for_compile;

    fn parse_root(source: &str) -> crate::ast::modern::Root {
        parse_component_for_compile(source)
            .expect("parse component")
            .root
    }

    #[test]
    fn renders_parenthesized_sequence_expression() {
        let root = parse_root("{@html (a, b)}");
        let Node::HtmlTag(tag) = &root.fragment.nodes[0] else {
            panic!("expected html tag");
        };
        assert_eq!(render(&tag.expression), Some(String::from("(a, b)")));
    }

    #[test]
    fn renders_optional_chain_render_expression() {
        let root = parse_root("{@render snippets?.[snippet]?.()}");
        let Node::RenderTag(tag) = &root.fragment.nodes[0] else {
            panic!("expected render tag");
        };
        assert_eq!(
            render(&tag.expression),
            Some(String::from("snippets?.[snippet]?.()"))
        );
    }

    #[test]
    fn renders_typed_snippet_parameter() {
        let root = parse_root("{#snippet row(item: Item)}{/snippet}");
        let Node::SnippetBlock(block) = &root.fragment.nodes[0] else {
            panic!("expected snippet block");
        };
        assert_eq!(
            render(&block.parameters[0]),
            Some(String::from("item: Item"))
        );
    }

    #[test]
    fn renders_common_statement_kinds_in_scripts() {
        let root = parse_root(
            "<script>if (ok) { throw new Error('boom'); } $: if (ready) value = 1; while (count) count -= 1; try { foo(); } catch (error) { bar(error); }</script>",
        );
        let program = root.instance.as_ref().expect("instance script");
        let body = estree_node_field_array(&program.content, RawField::Body).expect("body");
        let rendered = body
            .iter()
            .map(render)
            .collect::<Option<Vec<_>>>()
            .expect("render statements");

        assert_eq!(rendered[0], "if (ok) { throw new Error('boom'); }");
        assert_eq!(rendered[1], "$: if (ready) value = 1;");
        assert_eq!(rendered[2], "while (count) count -= 1;");
        assert_eq!(rendered[3], "try { foo(); } catch (error) { bar(error); }");
    }

    #[test]
    fn renders_class_fields_in_scripts() {
        let root = parse_root(
            "<script>class Counter { #count = $state(0); count = $derived(this.#count * 2); static { Counter.ready = true; } }</script>",
        );
        let program = root.instance.as_ref().expect("instance script");
        let body = estree_node_field_array(&program.content, RawField::Body).expect("body");
        let rendered = body
            .iter()
            .map(render)
            .collect::<Option<Vec<_>>>()
            .expect("render statements");

        assert_eq!(
            rendered[0],
            "class Counter { #count = $state(0); count = $derived(this.#count * 2); static {\n\tCounter.ready = true;\n} }"
        );
    }

    #[test]
    fn renders_readonly_class_fields_in_scripts() {
        let root = parse_root(
            "<script lang='ts'>class Counter { readonly ready = true; get count() { return this.ready ? 1 : 0; } }</script>",
        );
        let program = root.instance.as_ref().expect("instance script");
        let body = estree_node_field_array(&program.content, RawField::Body).expect("body");
        let rendered = body
            .iter()
            .map(render)
            .collect::<Option<Vec<_>>>()
            .expect("render statements");

        assert_eq!(
            rendered[0],
            "class Counter { readonly ready = true; get count() { return this.ready ? 1 : 0; } }"
        );
    }

    #[test]
    fn renders_generator_yield_in_scripts() {
        let root = parse_root("<script>function* count() { while (true) yield 1; }</script>");
        let program = root.instance.as_ref().expect("instance script");
        let body = estree_node_field_array(&program.content, RawField::Body).expect("body");
        let rendered = body
            .iter()
            .map(render)
            .collect::<Option<Vec<_>>>()
            .expect("render statements");

        assert_eq!(rendered[0], "function* count() { while (true) yield 1; }");
    }

    #[test]
    fn renders_typescript_declarations_in_scripts() {
        let root = parse_root(
            "<script lang='ts'>interface Hello { message: 'hello'; } type Goodbye = { message: 'goodbye' }; abstract class MyAbstractClass { abstract x(): void; y() {} } declare function declared_fn(): void; namespace SomeNamespace { export type Foo = true; }</script>",
        );
        let program = root.instance.as_ref().expect("instance script");
        let body = estree_node_field_array(&program.content, RawField::Body).expect("body");
        let rendered = body
            .iter()
            .map(render)
            .collect::<Option<Vec<_>>>()
            .expect("render statements");

        assert_eq!(rendered[0], "interface Hello { message: 'hello' }");
        assert_eq!(rendered[1], "type Goodbye = { message: 'goodbye' };");
        assert_eq!(
            rendered[2],
            "abstract class MyAbstractClass { abstract x(): void; y() {} }"
        );
        assert_eq!(rendered[3], "declare function declared_fn(): void;");
        assert_eq!(
            rendered[4],
            "namespace SomeNamespace { export type Foo = true; }"
        );
    }
}
