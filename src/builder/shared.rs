use super::builder::Builder;
use super::comment::JSDocComment;
use super::namespace::CppItem;
use super::traits::{ASTEntry, Access, EntityMethods, Entry, Include};
use crate::annotation::Annotations;
use crate::builder::namespace::Namespace;
use crate::config::Config;
use crate::html::{Html, HtmlElement, HtmlList, HtmlText};
use clang::{Accessibility, Entity, EntityKind, Type, TypeKind};
use multipeek::{IteratorExt, MultiPeek};
use pulldown_cmark::CowStr;
use std::str::Chars;
use std::sync::Arc;

trait Surround<T> {
    fn surround(self, start: T, end: T) -> Self;
}

impl<T> Surround<T> for Vec<T> {
    fn surround(mut self, start: T, end: T) -> Self {
        self.insert(0, start);
        self.push(end);
        self
    }
}

trait InsertBetween<T, Sep: Fn() -> T> {
    fn insert_between(self, separator: Sep) -> Self;
}

impl<T, Sep: Fn() -> T> InsertBetween<T, Sep> for Vec<T> {
    fn insert_between(self, separator: Sep) -> Self {
        let mut res = Vec::new();
        let mut first = true;
        for item in self.into_iter() {
            if !first {
                res.push(separator());
            }
            first = false;
            res.push(item);
        }
        res
    }
}

fn get_all_classes<'e>(namespace: &'e Namespace<'e>) -> Vec<&'e dyn ASTEntry<'e>> {
    namespace.get(&|entry| matches!(entry.category(), "class" | "struct"))
}

fn fmt_type(entity: &Type, builder: &Builder) -> Html {
    let base = entity.get_pointee_type().unwrap_or(entity.to_owned());
    let decl = base.get_declaration();
    let link = decl.and_then(|decl| decl.abs_docs_url(builder.config.clone()));
    let kind = decl
        .map(|decl| decl.get_kind())
        .unwrap_or(EntityKind::UnexposedDecl);

    let name: Html = decl
        .map(|decl| {
            HtmlList::new(
                decl.ancestorage()
                    .iter()
                    .map(|e| {
                        HtmlElement::new("span")
                            .with_class(match e.get_kind() {
                                EntityKind::Namespace => "namespace",
                                EntityKind::ClassDecl => "class",
                                EntityKind::ClassTemplate => "class",
                                EntityKind::StructDecl => "struct",
                                EntityKind::FunctionDecl => "fun",
                                EntityKind::TypedefDecl => "alias",
                                EntityKind::UsingDeclaration => "alias",
                                EntityKind::TypeAliasDecl => "alias",
                                EntityKind::EnumDecl => "enum",
                                _ => "type",
                            })
                            .with_class("name")
                            .with_child(HtmlText::new(e.get_name().unwrap_or("_".into())))
                            .into()
                    })
                    .collect::<Vec<_>>()
                    .insert_between(|| Html::span(&["scope"], "::")),
            )
            .into()
        })
        .unwrap_or_else(|| {
            HtmlElement::new("span")
                .with_class(if base.get_kind() == TypeKind::Unexposed {
                    "template-param"
                } else {
                    "keyword"
                })
                .with_class("name")
                .with_child(HtmlText::new(match base.get_kind() {
                    TypeKind::Void => "void".into(),
                    TypeKind::Bool => "bool".into(),
                    TypeKind::Long => "long".into(),
                    TypeKind::Auto => "auto".into(),
                    TypeKind::Int => "int".into(),
                    TypeKind::Short => "short".into(),
                    TypeKind::SChar | TypeKind::CharS => "char".into(),
                    TypeKind::UChar | TypeKind::CharU => "uchar".into(),
                    TypeKind::Float => "float".into(),
                    TypeKind::Double => "double".into(),
                    TypeKind::UInt => "uint".into(),
                    TypeKind::LongLong => "long long".into(),
                    _ => base.get_display_name(),
                }))
                .into()
        });

    HtmlElement::new("a")
        .with_class("entity")
        .with_class("type")
        .with_class_opt(entity.is_pod().then_some("keyword"))
        .with_class_opt(link.is_none().then_some("disabled"))
        .with_attr_opt("href", link.clone())
        .with_attr_opt(
            "onclick",
            link.map(|link| format!("return navigate('{link}')")),
        )
        .with_child(name)
        .with_child_opt(match kind {
            EntityKind::TypeAliasDecl | EntityKind::TypedefDecl => None,
            _ => base.get_template_argument_types().map(|types| {
                HtmlList::new(
                    types
                        .iter()
                        .map(|t| {
                            t.map(|t| fmt_type(&t, builder))
                                .unwrap_or(HtmlText::new("_unk").into())
                        })
                        .collect::<Vec<_>>()
                        .insert_between(|| {
                            HtmlElement::new("span")
                                .with_class("comma")
                                .with_class("space-after")
                                .with_child(HtmlText::new(","))
                                .into()
                        })
                        .surround(HtmlText::new("<").into(), HtmlText::new(">").into()),
                )
            }),
        })
        .with_child_opt(
            base.is_const_qualified()
                .then_some(Html::span(&["keyword", "space-before"], "const")),
        )
        .with_child_opt(match entity.get_kind() {
            TypeKind::LValueReference => Some::<Html>(HtmlText::new("&").into()),
            TypeKind::RValueReference => Some(HtmlText::new("&&").into()),
            TypeKind::Pointer => Some(HtmlText::new("*").into()),
            _ => None,
        })
        .into()
}

fn fmt_param(param: &Entity, builder: &Builder) -> Html {
    HtmlElement::new("div")
        .with_classes(&["entity", "var"])
        .with_child_opt(param.get_type().map(|t| fmt_type(&t, builder)))
        .with_child_opt(
            param
                .get_display_name()
                .map(|name| Html::span(&["name", "space-before"], &name)),
        )
        .into()
}

fn fmt_template_args(entity: &Entity, _builder: &Builder) -> Option<Html> {
    let template_children: Vec<Entity> = entity
        .get_children()
        .into_iter()
        .filter(|e| e.get_kind() == EntityKind::TemplateTypeParameter)
        .collect();
    if template_children.is_empty() {
        return None;
    }
    Some(
        HtmlElement::new("span")
            .with_class("template-params")
            .with_child(Html::span(&["keyword", "space-after"], "template"))
            .with_children(
                template_children
                    .into_iter()
                    .map(|e| {
                        HtmlText::new(
                            e.extract_source_string_cleaned()
                                .or_else(|| e.get_name().map(|x| format!("typename {x}")))
                                .unwrap_or("_".into()),
                        )
                        .into()
                    })
                    .collect::<Vec<_>>()
                    .insert_between(|| {
                        HtmlElement::new("span")
                            .with_class("comma")
                            .with_class("space-after")
                            .with_child(HtmlText::new(","))
                            .into()
                    })
                    .surround(HtmlText::new("<").into(), HtmlText::new(">").into()),
            )
            .into(),
    )
}

pub fn fmt_field(field: &Entity, builder: &Builder) -> Html {
    HtmlElement::new("details")
        .with_class("entity-desc")
        .with_child(
            HtmlElement::new("summary")
                .with_classes(&["entity", "var"])
                .with_child(fmt_param(field, builder))
                .with_child(HtmlText::new(";")),
        )
        .with_child(
            HtmlElement::new("div").with_child(
                field
                    .get_comment()
                    .map(|s| JSDocComment::parse(s, builder).to_html(true))
                    .unwrap_or(Html::span(&["no-desc"], "No description provided")),
            ),
        )
        .into()
}

fn fmt_fun_signature(fun: &Entity, builder: &Builder) -> Html {
    HtmlElement::new("summary")
        .with_classes(&["entity", "fun"])
        .with_child_opt(fmt_template_args(fun, builder))
        .with_child(
            HtmlElement::new("span")
                .with_class("function-signature")
                .with_child_opt(
                    fun.is_static_method()
                        .then_some(Html::span(&["keyword", "space-after"], "static")),
                )
                .with_child_opt(
                    fun.is_virtual_method()
                        .then_some(Html::span(&["keyword", "space-after"], "virtual")),
                )
                .with_child_opt(fun.get_result_type().map(|t| fmt_type(&t, builder)))
                .with_child(Html::span(
                    &["name", "space-before"],
                    &fun.get_name().unwrap_or("_anon".into()),
                ))
                .with_child(
                    HtmlElement::new("span").with_class("params").with_children(
                        fun.get_function_arguments()
                            .map(|args| {
                                args.iter()
                                    .map(|arg| fmt_param(arg, builder))
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or(Vec::new())
                            .insert_between(|| Html::span(&["comma", "space-after"], ","))
                            .surround(HtmlText::new("(").into(), HtmlText::new(")").into()),
                    ),
                )
                .with_child_opt(
                    fun.is_const_method()
                        .then_some(Html::span(&["keyword", "space-before"], "const")),
                )
                .with_child_opt(
                    fun.is_pure_virtual_method().then_some::<Html>(
                        HtmlList::new(vec![
                            Html::span(&["space-before"], "="),
                            Html::span(&["space-before", "literal"], "0"),
                        ])
                        .into(),
                    ),
                ),
        )
        .into()
}

pub fn fmt_class_method(fun: &Entity, builder: &Builder) -> Html {
    HtmlElement::new("details")
        .with_class("entity-desc")
        .with_attr_opt("id", member_fun_link(fun))
        .with_child(fmt_fun_signature(fun, builder))
        .with_child(
            HtmlElement::new("div").with_child(
                fun.get_comment()
                    .map(|s| JSDocComment::parse(s, builder).to_html(true))
                    .unwrap_or(Html::span(&["no-desc"], "No description provided")),
            ),
        )
        .into()
}

pub fn fmt_classlike_decl(class: &Entity, keyword: &str, builder: &Builder) -> Html {
    HtmlElement::new("details")
        .with_class("entity-desc")
        .with_child(
            HtmlElement::new("summary")
                .with_classes(&["entity", keyword])
                .with_child(Html::span(&["keyword", "space-after"], keyword))
                .with_child(Html::span(
                    &["name"],
                    &class.get_name().unwrap_or("_anon".into()),
                ))
                .with_child_opt(fmt_template_args(class, builder))
                .with_child(HtmlText::new(";")),
        )
        .with_child(
            HtmlElement::new("div").with_child(
                class
                    .get_comment()
                    .map(|s| JSDocComment::parse(s, builder).to_html(true))
                    .unwrap_or(Html::span(&["no-desc"], "No description provided")),
            ),
        )
        .into()
}

pub fn fmt_section(title: &str, data: Vec<Html>) -> Html {
    HtmlElement::new("details")
        .with_attr("open", "")
        .with_class("section")
        .with_child(
            HtmlElement::new("summary").with_child(
                HtmlElement::new("span")
                    .with_child(Html::feather("chevron-right"))
                    .with_child(HtmlText::new(title))
                    .with_child(Html::span(&["badge"], &data.len().to_string())),
            ),
        )
        .with_child(HtmlElement::new("div").with_child(HtmlList::new(data)))
        .into()
}

pub fn fmt_header_link(entity: &Entity, config: Arc<Config>) -> Html {
    if let Some(link) = entity.github_url(config.clone())
        && let Some(path) = entity.include_path(config.clone())
    {
        let exists_online = entity
            .config_source(config.clone())
            .map(|s| s.exists_online)
            .unwrap_or(true);
        let disabled = !exists_online;
        HtmlElement::new("a")
            .with_attr_opt("href", (!disabled).then_some(link))
            .with_class("header-link")
            .with_class_opt(disabled.then_some("disabled"))
            .with_child(
                HtmlElement::new("code")
                    .with_class("header-link")
                    .with_children(vec![
                        Html::span(&["keyword"], "#include "),
                        Html::span(&["url"], &format!("&lt;{}&gt;", path.to_raw_string())),
                    ]),
            )
            .into()
    } else {
        Html::p("&lt;Not available online&gt;")
    }
}

pub fn fmt_base_classes<'e, T: ASTEntry<'e>>(entry: &T, kw: &str, builder: &Builder) -> Html {
    let bases = entry
        .entity()
        .get_children()
        .into_iter()
        .filter(|p| p.get_kind() == EntityKind::BaseSpecifier)
        .collect::<Vec<_>>();

    HtmlElement::new("div")
        .with_classes(&["entity", "class"])
        .with_child_opt(fmt_template_args(entry.entity(), builder))
        .with_child(
            HtmlElement::new("span")
                .with_class("class-decl")
                .with_child(Html::span(&["keyword", "space-after"], kw))
                .with_child(Html::span(
                    &["name"],
                    entry.entity().get_name().unwrap_or("_".into()).as_str(),
                ))
                .with_child_opt(
                    (!bases.is_empty())
                        .then_some(Html::span(&["space-before", "space-after"], ":")),
                )
                .with_children(
                    bases
                        .into_iter()
                        .map(|base| {
                            HtmlList::new(
                                [
                                    base.get_accessibility().map(|a| {
                                        Html::span(
                                            &["keyword", "space-after"],
                                            match a {
                                                Accessibility::Public => "public",
                                                Accessibility::Private => "private",
                                                Accessibility::Protected => "protected",
                                            },
                                        )
                                    }),
                                    base.is_virtual_base().then_some(Html::span(
                                        &["keyword", "space-after"],
                                        "virtual",
                                    )),
                                    base.get_type().map(|ty| fmt_type(&ty, builder)),
                                ]
                                .into_iter()
                                .flatten()
                                .collect(),
                            )
                            .into()
                        })
                        .intersperse_with(|| Html::span(&["space-after"], ","))
                        .collect(),
                )
                .with_child(Html::span(&["space-before"], "{ ... }")),
        )
        .into()
}

pub fn fmt_derived_class(entity: &Entity, builder: &Builder) -> Html {
    HtmlElement::new("div")
        .with_classes(&["entity", "class"])
        .with_child(
            HtmlElement::new("a")
                .with_attr_opt("href", entity.abs_docs_url(builder.config.clone()))
                .with_child(Html::span(&["keyword", "space-after"], "class"))
                .with_child(Html::span(
                    &["name"],
                    entity.get_name().unwrap_or("_".into()).as_str(),
                ))
                .with_child(Html::span(&["space-before"], "{ ... }")),
        )
        .into()
}

pub fn output_entity<'e, T: ASTEntry<'e>>(
    entry: &T,
    builder: &Builder,
) -> Vec<(&'static str, Html)> {
    vec![
        ("name", HtmlText::new(entry.name()).into()),
        (
            "description",
            entry
                .entity()
                .get_comment()
                .map(|s| JSDocComment::parse(s, builder).to_html(false))
                .unwrap_or(Html::span(&["no-desc"], "No description provided")),
        ),
        (
            "header_link",
            fmt_header_link(entry.entity(), builder.config.clone()),
        ),
        (
            "examples",
            fmt_section(
                "Examples",
                entry
                    .entity()
                    .get_comment()
                    .map(|s| {
                        JSDocComment::parse(s, builder)
                            .examples()
                            .iter()
                            .map(|example| example.to_html())
                            .collect()
                    })
                    .unwrap_or(Vec::new()),
            ),
        ),
    ]
}

pub fn output_classlike<'e, T: ASTEntry<'e>>(
    entry: &T,
    builder: &Builder,
) -> Vec<(&'static str, Html)> {
    let mut ent = output_entity(entry, builder);
    ent.extend(vec![
        (
            "base_classes",
            fmt_base_classes(entry, entry.category(), builder),
        ),
        (
            "public_static_functions",
            fmt_section(
                "Public static methods",
                entry
                    .entity()
                    .get_member_functions(Access::Public, Include::Statics)
                    .into_iter()
                    .map(|e| fmt_class_method(&e, builder))
                    .collect::<Vec<_>>(),
            ),
        ),
        (
            "public_member_functions",
            fmt_section(
                "Public member functions",
                entry
                    .entity()
                    .get_member_functions(Access::Public, Include::Members)
                    .into_iter()
                    .map(|e| fmt_class_method(&e, builder))
                    .collect::<Vec<_>>(),
            ),
        ),
        (
            // todo: hide if final class
            "protected_member_functions",
            fmt_section(
                "Protected member functions",
                entry
                    .entity()
                    .get_member_functions(Access::Protected, Include::Members)
                    .into_iter()
                    .map(|e| fmt_class_method(&e, builder))
                    .collect::<Vec<_>>(),
            ),
        ),
        (
            "public_members",
            fmt_section(
                "Fields",
                entry
                    .entity()
                    .get_children()
                    .iter()
                    .filter(|child| {
                        child.get_kind() == EntityKind::FieldDecl
                            && child.get_accessibility() == Some(Accessibility::Public)
                    })
                    .map(|e| fmt_field(e, builder))
                    .collect::<Vec<_>>(),
            ),
        ),
        (
            "protected_members",
            fmt_section(
                "Protected fields",
                entry
                    .entity()
                    .get_children()
                    .iter()
                    .filter(|child| {
                        child.get_kind() == EntityKind::FieldDecl
                            && child.get_accessibility() == Some(Accessibility::Protected)
                    })
                    .map(|e| fmt_field(e, builder))
                    .collect::<Vec<_>>(),
            ),
        ),
        (
            "derived_classes",
            fmt_section("Derived classes", {
                let mut derived: Vec<_> = get_all_classes(&builder.root)
                    .into_iter()
                    .filter(|potentially_derived| {
                        potentially_derived
                            .entity()
                            .get_children()
                            .iter()
                            .any(|child| {
                                child.get_kind() == EntityKind::BaseSpecifier
                                    && child
                                        .get_type()
                                        .and_then(|t| t.get_declaration())
                                        .map(|decl| decl.get_usr() == entry.entity().get_usr())
                                        .unwrap_or(false)
                            })
                    })
                    .collect();
                derived.sort_by_key(|d| d.entity().get_name().unwrap_or("_".into()));
                derived
                    .into_iter()
                    .map(|d| fmt_derived_class(d.entity(), builder))
                    .collect::<Vec<_>>()
            }),
        ),
    ]);
    ent
}

pub fn output_function<'e, T: ASTEntry<'e>>(
    entry: &T,
    builder: &Builder,
) -> Vec<(&'static str, Html)> {
    let mut ent = output_entity(entry, builder);
    ent.extend(vec![(
        "function_signature",
        fmt_fun_signature(entry.entity(), builder),
    )]);
    ent
}

fn fmt_autolinks_recursive(
    entity: &CppItem,
    config: Arc<Config>,
    annotations: &mut Annotations<'_>,
) {
    annotations.rewind();
    while let Some(word) = annotations.next() {
        // skip stuff that have all-lowercase names (so words like "get"
        // and "data" don't get autolinked)
        if !word.chars().all(|c| c.is_lowercase()) && *word == entity.name() {
            if let Some(url) = entity.entity().abs_docs_url(config.clone()) {
                annotations.annotate(format!("[{word}]({})", url));
            }
        }
    }

    if let CppItem::Namespace(ns) = entity {
        for v in ns.entries.values() {
            fmt_autolinks_recursive(v, config.clone(), annotations);
        }
    }
}

pub fn fmt_autolinks(builder: &Builder, text: &str) -> String {
    let mut annotations = Annotations::new(text);
    for entry in builder.root.entries.values() {
        fmt_autolinks_recursive(entry, builder.config.clone(), &mut annotations);
    }
    annotations.into_result()
}

pub fn fmt_emoji(text: &CowStr) -> String {
    fn eat_emoji<'e>(iter: &mut MultiPeek<Chars>) -> Option<&'e str> {
        let mut buffer = String::new();
        let mut i = 0;
        while let Some(d) = iter.peek_nth(i) {
            if d.is_alphanumeric() || *d == '_' {
                buffer.push(*d);
            } else if *d == ':' {
                break;
            } else {
                return None;
            }
            i += 1;
        }
        if let Some(emoji) = emojis::get_by_shortcode(&buffer) {
            #[allow(clippy::match_single_binding)]
            match iter.advance_by(i + 1) {
                _ => {}
            }
            Some(emoji.as_str())
        } else {
            None
        }
    }

    let mut res = String::new();
    res.reserve(text.len());

    let mut iter = text.chars().multipeek();
    while let Some(c) = iter.next() {
        if c == ':'
            && let Some(emoji) = eat_emoji(&mut iter)
        {
            res.push_str(emoji);
        } else {
            res.push(c);
        }
    }

    res
}

pub fn member_fun_link(entity: &Entity) -> Option<String> {
    entity.get_name()
}
