use zed_extension_api::{
    CodeLabel, CodeLabelSpan, LanguageServerId,
    lsp::{Completion, CompletionKind, Symbol, SymbolKind},
};

pub fn label_for_completion(
    _language_server_id: &LanguageServerId,
    completion: Completion,
) -> Option<CodeLabel> {
    let kind = completion.kind?;
    let label = &completion.label;
    let detail = completion.detail.as_deref().unwrap_or("");

    match kind {
        // ZLS detail: "fn(params) ReturnType" → display: "fn name(params) ReturnType"
        CompletionKind::Function | CompletionKind::Method => {
            if let Some(rest) = detail.strip_prefix("fn") {
                let code = format!("fn {label}{rest} {{}}");
                let name_start = "fn ".len();
                let name_end = name_start + label.len();

                let mut spans = vec![
                    CodeLabelSpan::code_range(0.."fn ".len()),
                    CodeLabelSpan::code_range(name_start..name_end),
                ];

                let rest_trimmed = rest.trim();
                if !rest_trimmed.is_empty() {
                    spans.push(CodeLabelSpan::literal(
                        format!(" {rest_trimmed}"),
                        Some("variable".into()),
                    ));
                }

                return Some(CodeLabel {
                    code,
                    spans,
                    filter_range: (name_start..name_end).into(),
                });
            }

            simple_label(label)
        }

        // ZLS detail: "FieldType" → display: "name: FieldType"
        CompletionKind::Field => {
            if !detail.is_empty() {
                let code = format!("{label}: {detail},");
                let name_end = label.len();
                Some(CodeLabel {
                    spans: vec![
                        CodeLabelSpan::code_range(0..name_end),
                        CodeLabelSpan::literal(format!(": {detail}"), Some("type".into())),
                    ],
                    code,
                    filter_range: (0..name_end).into(),
                })
            } else {
                simple_label(label)
            }
        }

        // ZLS detail: "const T" / "var T" / "T" → display: "const name: T" etc.
        CompletionKind::Variable | CompletionKind::Constant => {
            let (kw, type_str) = if let Some(t) = detail.strip_prefix("const ") {
                ("const ", t)
            } else if let Some(t) = detail.strip_prefix("var ") {
                ("var ", t)
            } else {
                ("", detail)
            };

            if !type_str.is_empty() {
                let code = format!("{kw}{label}: {type_str};");
                let name_start = kw.len();
                let name_end = name_start + label.len();
                Some(CodeLabel {
                    spans: vec![
                        CodeLabelSpan::code_range(0..kw.len()),
                        CodeLabelSpan::code_range(name_start..name_end),
                        CodeLabelSpan::literal(format!(": {type_str}"), Some("type".into())),
                    ],
                    code,
                    filter_range: (name_start..name_end).into(),
                })
            } else {
                simple_label(label)
            }
        }

        // ZLS detail: "EnumTypeName" → display: "EnumTypeName.name"
        CompletionKind::EnumMember => {
            if !detail.is_empty() {
                let code = format!("const {detail} = .{label};");
                let prefix = format!("const {detail} = .");
                let name_start = prefix.len();
                let name_end = name_start + label.len();
                Some(CodeLabel {
                    spans: vec![
                        CodeLabelSpan::literal(format!("{detail}."), Some("type".into())),
                        CodeLabelSpan::code_range(name_start..name_end),
                    ],
                    code,
                    filter_range: (0..label.len()).into(),
                })
            } else {
                simple_label(label)
            }
        }

        CompletionKind::Struct
        | CompletionKind::Class
        | CompletionKind::Interface
        | CompletionKind::Enum => {
            let kw = match kind {
                CompletionKind::Interface => "union ",
                _ => "const ",
            };
            let code = format!("{kw}{label} = struct {{}};");
            let name_start = kw.len();
            let name_end = name_start + label.len();
            Some(CodeLabel {
                spans: vec![CodeLabelSpan::code_range(name_start..name_end)],
                code,
                filter_range: (name_start..name_end).into(),
            })
        }

        CompletionKind::Keyword => Some(CodeLabel {
            spans: vec![CodeLabelSpan::code_range(0..label.len())],
            filter_range: (0..label.len()).into(),
            code: label.clone(),
        }),

        CompletionKind::Module | CompletionKind::Unit => {
            let code = format!("const {label} = @import(\"\");");
            let name_start = "const ".len();
            let name_end = name_start + label.len();
            Some(CodeLabel {
                spans: vec![
                    CodeLabelSpan::code_range(0.."const ".len()),
                    CodeLabelSpan::code_range(name_start..name_end),
                ],
                code,
                filter_range: (name_start..name_end).into(),
            })
        }

        _ => simple_label(label),
    }
}

pub fn label_for_symbol(
    _language_server_id: &LanguageServerId,
    symbol: Symbol,
) -> Option<CodeLabel> {
    let name = &symbol.name;

    match symbol.kind {
        SymbolKind::Function | SymbolKind::Method => {
            let code = format!("fn {name}() {{}}");
            let start = "fn ".len();
            Some(CodeLabel {
                spans: vec![
                    CodeLabelSpan::code_range(0.."fn ".len()),
                    CodeLabelSpan::code_range(start..start + name.len()),
                ],
                filter_range: (start..start + name.len()).into(),
                code,
            })
        }

        SymbolKind::Struct | SymbolKind::Class => {
            let code = format!("const {name} = struct {{}};");
            let start = "const ".len();
            Some(CodeLabel {
                spans: vec![CodeLabelSpan::code_range(start..start + name.len())],
                filter_range: (start..start + name.len()).into(),
                code,
            })
        }

        SymbolKind::Enum => {
            let code = format!("const {name} = enum {{}};");
            let start = "const ".len();
            Some(CodeLabel {
                spans: vec![CodeLabelSpan::code_range(start..start + name.len())],
                filter_range: (start..start + name.len()).into(),
                code,
            })
        }

        SymbolKind::Interface => {
            let code = format!("const {name} = union {{}};");
            let start = "const ".len();
            Some(CodeLabel {
                spans: vec![CodeLabelSpan::code_range(start..start + name.len())],
                filter_range: (start..start + name.len()).into(),
                code,
            })
        }

        SymbolKind::Constant | SymbolKind::Variable | SymbolKind::Field => {
            let code = format!("const {name}: T = undefined;");
            let start = "const ".len();
            Some(CodeLabel {
                spans: vec![CodeLabelSpan::code_range(start..start + name.len())],
                filter_range: (start..start + name.len()).into(),
                code,
            })
        }

        SymbolKind::Module | SymbolKind::Namespace | SymbolKind::Package => {
            let code = format!("const {name} = @import(\"\");");
            let start = "const ".len();
            Some(CodeLabel {
                spans: vec![
                    CodeLabelSpan::code_range(0.."const ".len()),
                    CodeLabelSpan::code_range(start..start + name.len()),
                ],
                filter_range: (start..start + name.len()).into(),
                code,
            })
        }

        _ => None,
    }
}

fn simple_label(label: &str) -> Option<CodeLabel> {
    Some(CodeLabel {
        code: label.to_string(),
        spans: vec![CodeLabelSpan::code_range(0..label.len())],
        filter_range: (0..label.len()).into(),
    })
}
