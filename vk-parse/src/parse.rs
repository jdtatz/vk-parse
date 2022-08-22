extern crate xml;

use std;
use std::io::Read;
use std::ops::ControlFlow;
use std::str::FromStr;
use xml::attribute::OwnedAttribute;
use xml::name::OwnedName;
use xml::reader::XmlEvent;

use types::*;

type XmlEvents<R> = xml::reader::Events<R>;
type XmlAttribute = xml::attribute::OwnedAttribute;

//--------------------------------------------------------------------------------------------------
struct ParseCtx<R: Read> {
    events: XmlEvents<R>,
    xpath: String,
    errors: Vec<Error>,
}

impl<R: Read> ParseCtx<R> {
    fn push_element(&mut self, name: &str) {
        self.xpath.push('/');
        self.xpath.push_str(name);
    }

    fn pop_element(&mut self) {
        if let Some(separator_pos) = self.xpath.rfind('/') {
            self.xpath.truncate(separator_pos);
        } else {
            self.errors.push(Error::Internal {
                desc: "ParseCtx push_element/pop_element mismatch.",
            });
        }
    }
}

fn xpath_attribute(xpath: &str, attribute_name: &str) -> String {
    let mut xpath = String::from(xpath);
    xpath.push_str("[@");
    xpath.push_str(attribute_name);
    xpath.push(']');
    xpath
}

//--------------------------------------------------------------------------------------------------
macro_rules! unwrap_attribute (
    ($ctx:expr, $element:ident, $attribute:ident) => {
        let $attribute = match $attribute {
            Some(val) => val,
            None => {
                $ctx.errors.push(Error::MissingAttribute {
                    xpath: $ctx.xpath.clone(),
                    name: String::from(stringify!($attribute)),
                });
                return None;
            }
        };
    };
);

macro_rules! match_attributes {
    ($ctx:expr, $a:ident in $attributes:expr, $($p:pat => $e:expr),+ $(,)?) => {
        for $a in $attributes {
            let n = $a.name.local_name.as_str();
            match n {
                $(
                    $p => $e,
                )+
                _ => $ctx.errors.push(Error::UnexpectedAttribute {
                    xpath: $ctx.xpath.clone(),
                    name: String::from(n),
                })
            }
        }
    };
}

macro_rules! match_elements {
    ($ctx:expr, $($p:pat => $e:expr),+) => {
        while let Some(Ok(e)) = $ctx.events.next() {
            match e {
                XmlEvent::StartElement { name, .. } => {
                    let name = name.local_name.as_str();
                    $ctx.push_element(name);
                    match name {
                        $(
                            $p => $e,
                        )+
                        _ => {
                            $ctx.errors.push(Error::UnexpectedElement {
                                xpath: $ctx.xpath.clone(),
                                name: String::from(name),
                            });
                            consume_current_element($ctx);
                        }
                    }
                }
                XmlEvent::EndElement { .. } => {
                    $ctx.pop_element();
                    break;
                }
                _ => {}
            }
        }
    };

    ( $ctx:expr, $attributes:ident, $($p:pat => $e:expr),+) => {
        while let Some(Ok(e)) = $ctx.events.next() {
            match e {
                XmlEvent::StartElement { name, $attributes, .. } => {
                    let name = name.local_name.as_str();
                    $ctx.push_element(name);
                    match name {
                        $(
                            $p => $e,
                        )+
                        _ => {
                            $ctx.errors.push(Error::UnexpectedElement {
                                xpath: $ctx.xpath.clone(),
                                name: String::from(name),
                            });
                            consume_current_element($ctx);
                        }
                    }
                }
                XmlEvent::EndElement { .. } => {
                    $ctx.pop_element();
                    break;
                }
                _ => {}
            }
        }
    };
}

macro_rules! match_elements_combine_text {
    ( $ctx:expr, $attributes:ident, $buffer:ident, $($p:pat $(if $g:expr)? => $e:expr),+) => {
        while let Some(Ok(e)) = $ctx.events.next() {
            match e {
                XmlEvent::Characters(text) => $buffer.push_str(&text),
                XmlEvent::Whitespace(text) => $buffer.push_str(&text),
                XmlEvent::StartElement { name, attributes: $attributes, .. } => {
                    let name = name.local_name.as_str();
                    $ctx.push_element(name);
                    match name {
                        $(
                            $p $(if $g)? => $e,
                        )+
                        _ => {
                            $ctx.errors.push(Error::UnexpectedElement {
                                xpath: $ctx.xpath.clone(),
                                name: String::from(name),
                            });
                            consume_current_element($ctx);
                        }
                    }
                }
                XmlEvent::EndElement { .. } => {
                    $ctx.pop_element();
                    break;
                }
                _ => {}
            }
        }
    };

    ( $ctx:expr, $buffer:ident, $($p:pat => $e:expr),+) => {
        match_elements_combine_text!{ $ctx, _attributes, $buffer, $($p => $e),+ }
    };
}

//--------------------------------------------------------------------------------------------------
/// Parses the Vulkan XML file into a Rust object.
pub fn parse_file(path: &std::path::Path) -> Result<(Registry, Vec<Error>), FatalError> {
    let file = std::io::BufReader::new(std::fs::File::open(path)?);
    let parser = xml::reader::ParserConfig::new().create_reader(file);
    parse_xml(parser.into_iter())
}

/// Parses the Vulkan XML file from stream into a Rust object.
pub fn parse_stream<T: std::io::Read>(stream: T) -> Result<(Registry, Vec<Error>), FatalError> {
    let parser = xml::reader::ParserConfig::new().create_reader(stream);
    parse_xml(parser.into_iter())
}

fn parse_xml<R: Read>(events: XmlEvents<R>) -> Result<(Registry, Vec<Error>), FatalError> {
    let mut ctx = ParseCtx {
        events,
        xpath: String::from(""),
        errors: Vec::new(),
    };

    let mut result = Err(FatalError::MissingRegistryElement);

    {
        let ctx = &mut ctx;
        match_elements! {ctx,
            "registry" => result = parse_registry(ctx)
        }
    }

    result.map(|r| (r, ctx.errors))
}

fn parse_registry<R: Read>(ctx: &mut ParseCtx<R>) -> Result<Registry, FatalError> {
    let mut registry = Registry(Vec::new());

    match_elements! {ctx, attributes,
        "comment" => registry.0.push(RegistryChild::Comment(parse_text_element(ctx))),
        "vendorids" => registry.0.push(parse_vendorids(ctx, attributes)),
        "platforms" => {
            let mut comment = None;
            let mut children = Vec::new();

            match_attributes!{ctx, a in attributes,
                "comment" => comment = Some(a.value)
            }

            match_elements!{ctx, attributes,
                "platform" => if let Some(v) = parse_platform(ctx, attributes) {
                    children.push(v);
                }
            }

            registry.0.push(RegistryChild::Platforms(Platforms { comment, children }));
        },

        "tags" => registry.0.push(parse_tags(ctx, attributes)),
        "types" => {
            let mut comment = None;
            let mut children = Vec::new();
            match_attributes!{ctx, a in attributes,
                "comment" => comment = Some(a.value)
            }
            match_elements!{ctx, attributes,
                "comment" => children.push(TypesChild::Comment(parse_text_element(ctx))),
                "type" => children.push(parse_type(ctx, attributes))
            }
            registry.0.push(RegistryChild::Types(Types{
                comment,
                children
            }));
        },
        "enums" => {
            let mut name = None;
            let mut kind = None;
            let mut start = None;
            let mut end = None;
            let mut vendor = None;
            let mut comment = None;
            let mut bitwidth = None;
            let mut children = Vec::new();
            match_attributes!{ctx, a in attributes,
                "name"     => name     = Some(a.value),
                "type"     => kind     = Some(a.value),
                "start"    => start    = Some(a.value),
                "end"      => end      = Some(a.value),
                "vendor"   => vendor   = Some(a.value),
                "comment"  => comment  = Some(a.value),
                "bitwidth" => bitwidth = Some(a.value)
            }
            match_elements!{ctx, attributes,
                "enum" => if let Some(v) = parse_enum(ctx, attributes) {
                    children.push(EnumsChild::Enum(v));
                },
                "unused" => if let Some(v) = parse_enums_child_unused(ctx, attributes) {
                    children.push(v);
                },
                "comment" => children.push(EnumsChild::Comment(parse_text_element(ctx)))
            }

            let start = start.and_then(|val| parse_integer(ctx, &val));
            let end = end.and_then(|val| parse_integer(ctx, &val));
            let bitwidth = bitwidth.and_then(|val| parse_integer(ctx, &val)).map(|val| val as u32);

            registry.0.push(RegistryChild::Enums(Enums{ name, kind, start, end, vendor, comment, children, bitwidth }));
        },
        "commands" => {
            let mut comment = None;
            let mut children = Vec::new();

            match_attributes!{ctx, a in attributes,
                "comment" => comment = Some(a.value)
            }

            match_elements!{ctx, attributes,
                "command" => if let Some(v) = parse_command(ctx, attributes) {
                    children.push(v);
                }
            }

            registry.0.push(RegistryChild::Commands(Commands{comment, children}));
        },
        "feature" => if let Some(v) = parse_feature(ctx, attributes) {
            registry.0.push(v);
        },
        "extensions" => registry.0.push(parse_extensions(ctx, attributes)),
        "formats" => registry.0.push(parse_formats(ctx)),
        "spirvextensions" => registry.0.push(parse_spirvextensions(ctx, attributes)),
        "spirvcapabilities" => registry.0.push(parse_spirvcapabilities(ctx, attributes))
    }

    Ok(registry)
}

fn parse_vendorids<R: Read>(ctx: &mut ParseCtx<R>, attributes: Vec<XmlAttribute>) -> RegistryChild {
    let mut comment = None;
    let mut children = Vec::new();

    match_attributes! {ctx, a in attributes,
        "comment" => comment = Some(a.value)
    }

    match_elements! {ctx, attributes,
        "vendorid" => if let Some(v) = parse_vendorid(ctx, attributes) {
            children.push(v);
        }
    }

    RegistryChild::VendorIds(VendorIds { comment, children })
}

fn parse_vendorid<R: Read>(
    ctx: &mut ParseCtx<R>,
    attributes: Vec<XmlAttribute>,
) -> Option<VendorId> {
    let mut name = None;
    let mut comment = None;
    let mut id = None;

    match_attributes! {ctx, a in attributes,
        "name" => name = Some(a.value),
        "comment" => comment = Some(a.value),
        "id" => {
            let mut v = None;
            if a.value.starts_with("0x") {
                v = u32::from_str_radix(&a.value.split_at(2).1, 16).ok();
            }

            if let Some(v) = v {
                id = Some(v);
            } else {
                ctx.errors.push(Error::UnexpectedAttributeValue {
                    xpath: ctx.xpath.clone(),
                    name: String::from("id"),
                    value: a.value.clone(),
                });
            }
        }
    }

    consume_current_element(ctx);

    unwrap_attribute!(ctx, vendorid, name);
    unwrap_attribute!(ctx, vendorid, id);

    Some(VendorId { name, comment, id })
}

fn parse_platform<R: Read>(
    ctx: &mut ParseCtx<R>,
    attributes: Vec<XmlAttribute>,
) -> Option<Platform> {
    let mut name = None;
    let mut comment = None;
    let mut protect = None;

    match_attributes! {ctx, a in attributes,
        "name"    => name    = Some(a.value),
        "comment" => comment = Some(a.value),
        "protect" => protect = Some(a.value)
    }

    consume_current_element(ctx);

    unwrap_attribute!(ctx, platform, name);
    unwrap_attribute!(ctx, platform, protect);

    Some(Platform {
        name,
        comment,
        protect,
    })
}

fn parse_tags<R: Read>(ctx: &mut ParseCtx<R>, attributes: Vec<XmlAttribute>) -> RegistryChild {
    let mut comment = None;
    let mut children = Vec::new();

    match_attributes! {ctx, a in attributes,
        "comment" => comment = Some(a.value)
    }

    match_elements! {ctx, attributes,
        "tag" => if let Some(v) = parse_tag(ctx, attributes) {
            children.push(v);
        }
    }

    RegistryChild::Tags(Tags { comment, children })
}

fn parse_tag<R: Read>(ctx: &mut ParseCtx<R>, attributes: Vec<XmlAttribute>) -> Option<Tag> {
    let mut name = None;
    let mut author = None;
    let mut contact = None;

    match_attributes! {ctx, a in attributes,
        "name"    => name    = Some(a.value),
        "author"  => author  = Some(a.value),
        "contact" => contact = Some(a.value)
    }

    consume_current_element(ctx);

    unwrap_attribute!(ctx, tag, name);
    unwrap_attribute!(ctx, tag, author);
    unwrap_attribute!(ctx, tag, contact);

    Some(Tag {
        name,
        author,
        contact,
    })
}

#[derive(Debug, Default, Clone, Copy)]
struct ParsedPreTypeTag {
    is_const: bool,
    is_struct: bool,
}

fn parse_pre_type_tag_text(text: &str) -> ParsedPreTypeTag {
    // handle const_ptr / struct info, can be 'const', 'struct', or 'const struct'
    let trimmed_text = text.trim();
    let (is_const, trimmed_text) = trimmed_text
        .strip_prefix("const")
        .map_or((false, trimmed_text), |t| (true, t.trim_start()));
    let (is_struct, trimmed_text) = trimmed_text
        .strip_prefix("struct")
        .map_or((false, trimmed_text), |t| (true, t.trim_start()));
    assert_eq!(trimmed_text, "");
    ParsedPreTypeTag {
        is_const,
        is_struct,
    }
}

fn parse_post_type_tag_text(
    text: &str,
    parsed_pre: ParsedPreTypeTag,
) -> (Option<PointerKind>, &str) {
    let ParsedPreTypeTag { is_const, .. } = parsed_pre;
    // handle pointer info, can be '*' or '**' or '* const*'
    let trimmed_text = text.trim();
    if let Some(trimmed_text) = trimmed_text.strip_prefix('*') {
        let trimmed_text = trimmed_text.trim_start();
        if let Some(trimmed_text) = trimmed_text.strip_prefix("const") {
            (
                Some(PointerKind::Double {
                    is_const,
                    inner_is_const: true,
                }),
                trimmed_text
                    .trim_start()
                    .strip_prefix('*')
                    .expect("In post type tag text a '* const' must be followed by '*'")
                    .trim_start(),
            )
        } else if let Some(trimmed_text) = trimmed_text.strip_prefix('*') {
            (
                Some(PointerKind::Double {
                    is_const,
                    inner_is_const: false,
                }),
                trimmed_text.trim_start(),
            )
        } else {
            (Some(PointerKind::Single { is_const }), trimmed_text)
        }
    } else {
        (None, trimmed_text)
    }
}

fn parse_array_shape_text(text: &str, shape_vec: &mut Vec<ArrayLength>) -> bool {
    if let Some(mut trimmed_text) = text.trim().strip_prefix('[') {
        while let Some((n, rest)) = trimmed_text.split_once(']') {
            shape_vec.push(ArrayLength::Static(n.parse().unwrap()));
            if let Some(rest) = rest.trim_start().strip_prefix('[') {
                trimmed_text = rest;
            } else {
                assert_eq!(rest, "");
                return false;
            }
        }
        true
    } else {
        false
    }
}

fn parse_name_with_type<
    R: Read,
    F: FnMut(&mut ParseCtx<R>, &mut String, &str, &[OwnedAttribute]) -> ControlFlow<()>,
>(
    ctx: &mut ParseCtx<R>,
    code: &mut String,
    len: Option<String>,
    altlen: Option<String>,
    externsync: Option<String>,
    optional: Option<String>,
    noautovalidity: Option<String>,
    objecttype: Option<String>,
    mut handle_extra: F,
) -> Option<NameWithType> {
    let dynamic_shape = if let Some(latex_expr) =
        len.as_deref().and_then(|l| l.strip_prefix("latexmath:"))
    {
        // altlen was only added in version 61, and we support down to version 33
        // let c_expr = altlen.expect("The `altlen` attribute is required when the `len` attribute is a latex expression");

        if let Some(c_expr) = altlen {
            Some(DynamicShapeKind::Expression {
                latex_expr: Some(latex_expr.to_string()),
                c_expr,
            })
        } else {
            None
        }
    } else if let Some(c_expr) = altlen {
        // only required/fixed in version >= 1.2.188(?)
        // unreachable!("only expecting the `altlen` attribute when the `len` attribute is a latex expression");
        Some(DynamicShapeKind::Expression {
            latex_expr: None,
            c_expr,
        })
    } else if let Some(len) = len {
        let mut it = len.split(',').map(|v| {
            if v == "null-terminated" {
                DynamicLength::NullTerminated
            } else if let Ok(n) = v.parse() {
                DynamicLength::Static(n)
            } else if let Some((parameter, field)) = v.split_once("->") {
                DynamicLength::ParameterizedField {
                    parameter: parameter.to_string(),
                    field: field.to_string(),
                }
            } else {
                DynamicLength::Parameterized(v.to_string())
            }
        });
        let outer = it.next().expect("The `len` attribute must not be empty");
        Some(match it.next() {
            Some(inner) => {
                let n = it.count();
                if n != 0 {
                    panic!(
                        "Expected only 1 or 2 comma-seperated values in the `len` attribute, found {} values",
                        2 + n
                    );
                }
                DynamicShapeKind::Double(outer, inner)
            }
            None => DynamicShapeKind::Single(outer),
        })
    } else {
        None
    };
    let optional = optional.as_deref().map(|v| {
        let mut it = v.split(',').map(|v| match v.parse::<bool>() {
            Err(_) => panic!("The comma-seperated values of the `optional` attribute must be either \"true\" | \"false\" not {:?}", v),
            Ok(b) => b,
        });
        let outer = it.next().expect("The `optional` attribute must not be empty");
        match it.next() {
            Some(inner) => {
                // broken in versions 1.2.162,1.2.163, & 1.2.164, see https://github.com/KhronosGroup/Vulkan-Docs/issues/1405
                // assert_eq!(it.count(), 0, "Expected only 1 or 2 comma-seperated values in the `optional` attribute");
                OptionalKind::Double(outer, inner)
            },
            None => OptionalKind::Single(outer)
        }
    });
    let noautovalidity = match noautovalidity.as_deref() {
        Some("true") => Some(()),
        None => None,
        Some(v) => panic!(
            "The `noautovalidity` attribute must only be \"true\" and not {:?}",
            v
        ),
    };
    let mut event = ctx.events.next();
    if let Some(Ok(XmlEvent::Whitespace(text))) = event {
        code.push_str(&text);
        event = ctx.events.next();
    }
    let parsed_pre = if let Some(Ok(XmlEvent::Characters(text))) = event {
        code.push_str(&text);
        event = ctx.events.next();

        parse_pre_type_tag_text(&text)
    } else {
        Default::default()
    };

    let type_name = match event {
        Some(Ok(XmlEvent::StartElement {
            name: OwnedName { local_name, .. },
            ..
        })) if local_name == "type" => {
            ctx.push_element(&local_name);
            let text = parse_text_element(ctx);
            code.push_str(&text);
            event = ctx.events.next();

            text
        }
        _ => {
            ctx.errors.push(Error::MissingElement {
                xpath: ctx.xpath.clone(),
                name: String::from("type"),
            });
            return None;
        }
    };

    if let Some(Ok(XmlEvent::Whitespace(text))) = event {
        code.push_str(&text);
        event = ctx.events.next();
    }
    let pointer_kind = if let Some(Ok(XmlEvent::Characters(text))) = event {
        code.push_str(&text);
        event = ctx.events.next();

        let (kind, rest) = parse_post_type_tag_text(&text, parsed_pre);
        assert_eq!(rest, "");
        kind
    } else {
        None
    };

    let name = match event {
        Some(Ok(XmlEvent::StartElement {
            name: OwnedName { local_name, .. },
            ..
        })) if local_name == "name" => {
            ctx.push_element(&local_name);
            let text = parse_text_element(ctx);
            code.push_str(&text);
            event = ctx.events.next();

            text
        }
        _ => {
            ctx.errors.push(Error::MissingElement {
                xpath: ctx.xpath.clone(),
                name: String::from("name"),
            });
            return None;
        }
    };

    let (bitfield_size, mut array_shape, mut hungry) =
        if let Some(Ok(XmlEvent::Characters(text))) = event {
            code.push_str(&text);
            event = ctx.events.next();

            let trimmed_text = text.trim();
            //  handle bitfield / statically-sized arrays
            if let Some(rest) = trimmed_text.strip_prefix(':') {
                (
                    Some(
                        rest.parse()
                            .expect("after bitfield's ':' only a non-zero integer is expected"),
                    ),
                    None,
                    false,
                )
            }
            // TODO is there anything else that could be here
            else {
                let mut shape = Vec::new();
                let hungry = parse_array_shape_text(trimmed_text, &mut shape);
                (None, Some(shape), hungry)
            }
        } else {
            (None, None, false)
        };

    while hungry {
        let array_shape_vec = array_shape.as_mut().unwrap();
        //
        match event {
            Some(Ok(XmlEvent::StartElement {
                name: OwnedName { local_name, .. },
                ..
            })) if local_name == "enum" => {
                ctx.push_element(&local_name);
                let text = parse_text_element(ctx);
                code.push_str(&text);
                event = ctx.events.next();

                array_shape_vec.push(ArrayLength::Constant(text));
            }
            _ => {
                ctx.errors.push(Error::MissingElement {
                    xpath: ctx.xpath.clone(),
                    name: String::from("enum"),
                });
                return None;
            }
        };
        //
        if let Some(Ok(XmlEvent::Characters(text))) = event {
            code.push_str(&text);
            event = ctx.events.next();

            let trimmed_text = text
                .trim()
                .strip_prefix("]")
                .expect("Expected a ']' to denote the end of an element of a shape");
            hungry = parse_array_shape_text(trimmed_text.trim_start(), array_shape_vec);
        } else {
            ctx.errors.push(Error::MissingCharacters {
                xpath: ctx.xpath.clone(),
            });
            return None;
        };
    }

    while let Some(Ok(e)) = event {
        match e {
            XmlEvent::Whitespace(text) => code.push_str(&text),
            XmlEvent::Characters(text) => code.push_str(&text),
            XmlEvent::StartElement {
                name: elem_name,
                attributes,
                ..
            } => {
                let elem_name = elem_name.local_name.as_str();
                ctx.push_element(elem_name);
                if let ControlFlow::Break(()) =
                    handle_extra(ctx, code, elem_name, attributes.as_ref())
                {
                    ctx.errors.push(Error::UnexpectedElement {
                        xpath: ctx.xpath.clone(),
                        name: String::from(elem_name),
                    });
                    consume_current_element(ctx);
                }
            }
            XmlEvent::EndElement { .. } => {
                ctx.pop_element();
                break;
            }
            _ => {}
        }
        event = ctx.events.next();
    }

    let externsync = externsync.map(|v| {
        if v == "true" {
            ExternSyncKind::Value
        } else {
            let it = v.split(',').map(|value| {
                let v = value.strip_prefix(name.as_str()).expect("");
                if let Some(field) = v.strip_prefix("->") {
                    field
                } else if let Some(field) = v.strip_prefix("[].") {
                    field
                } else if let Some(field) = v.strip_prefix(".").or_else(|| v.strip_prefix("::")) {
                    // these field seperators were phased out / fixed by version 138, and replaced with '->'
                    // https://github.com/KhronosGroup/Vulkan-Docs/pull/1222
                    field
                } else {
                    unreachable!("unspported field sperator found {:?}", value)
                }
                .to_string()
            });
            ExternSyncKind::Fields(it.collect())
        }
    });

    Some(NameWithType {
        type_name,
        pointer_kind,
        is_struct: parsed_pre.is_struct,
        bitfield_size,
        name,
        array_shape,
        dynamic_shape,
        externsync,
        optional,
        noautovalidity,
        objecttype,
    })
}

fn parse_type_funcptr<R: Read>(ctx: &mut ParseCtx<R>) -> Option<TypeFunctionPointer> {
    let (type_name, pointer_kind) = if let Some(Ok(XmlEvent::Characters(text))) = ctx.events.next()
    {
        let trimmed_text = text.trim();
        let trimmed_text = trimmed_text.strip_prefix("typedef").expect("").trim_start();
        let trimmed_text = trimmed_text.strip_suffix('*').expect("").trim_end();
        let trimmed_text = trimmed_text.strip_suffix("VKAPI_PTR").expect("").trim_end();
        let type_str = trimmed_text.strip_suffix('(').expect("").trim_end();
        // FIXME `type_str` can be any C type and needs full parsing, but for now just assuming is either a basic type or a non-const pointer to one
        let (type_str, pointer_kind) =
            type_str.strip_suffix('*').map_or((type_str, None), |rest| {
                (
                    rest.trim_end(),
                    Some(PointerKind::Single { is_const: false }),
                )
            });
        (type_str.to_string(), pointer_kind)
    } else {
        ctx.errors.push(Error::MissingCharacters {
            xpath: ctx.xpath.clone(),
        });
        return None;
    };

    let name = match ctx.events.next() {
        Some(Ok(XmlEvent::StartElement {
            name: OwnedName { local_name, .. },
            ..
        })) if local_name == "name" => {
            ctx.push_element(&local_name);
            let text = parse_text_element(ctx);

            text
        }
        _ => {
            ctx.errors.push(Error::MissingElement {
                xpath: ctx.xpath.clone(),
                name: String::from("name"),
            });
            return None;
        }
    };

    let fnptr_defn = NameWithType {
        name,
        type_name,
        pointer_kind,
        is_struct: false,
        bitfield_size: None,
        array_shape: None,
        dynamic_shape: None,
        externsync: None,
        optional: None,
        noautovalidity: None,
        objecttype: None,
    };

    let mut params = Vec::new();

    let mut parsed_pre = if let Some(Ok(XmlEvent::Characters(text))) = ctx.events.next() {
        let trimmed_text = text.trim();
        // empty params will have text be `)(void);`
        if trimmed_text.ends_with(';') {
            return Some(TypeFunctionPointer {
                proto: fnptr_defn,
                params,
            });
        }

        let trimmed_text = trimmed_text.strip_prefix(')').expect("").trim_start();
        let trimmed_text = trimmed_text.strip_prefix('(').expect("").trim_start();
        parse_pre_type_tag_text(trimmed_text)
    } else {
        ctx.errors.push(Error::MissingCharacters {
            xpath: ctx.xpath.clone(),
        });
        return None;
    };

    loop {
        let type_name = match ctx.events.next() {
            Some(Ok(XmlEvent::StartElement {
                name: OwnedName { local_name, .. },
                ..
            })) if local_name == "type" => {
                ctx.push_element(&local_name);
                let text = parse_text_element(ctx);

                text
            }
            _ => {
                ctx.errors.push(Error::MissingElement {
                    xpath: ctx.xpath.clone(),
                    name: String::from("type"),
                });
                return None;
            }
        };

        if let Some(Ok(XmlEvent::Characters(text))) = ctx.events.next() {
            let (pointer_kind, rest) = parse_post_type_tag_text(&text, parsed_pre);

            let (name, rest) = rest.split_once(',').map_or_else(
                || (rest.strip_suffix(");").unwrap_or(rest), None),
                |(n, r)| (n, Some(r)),
            );
            params.push(NameWithType {
                type_name,
                pointer_kind,
                is_struct: parsed_pre.is_struct,
                bitfield_size: None,
                array_shape: None,
                name: name.to_string(),
                dynamic_shape: None,
                externsync: None,
                optional: None,
                noautovalidity: None,
                objecttype: None,
            });
            if let Some(rest) = rest {
                parsed_pre = parse_pre_type_tag_text(rest.trim());
            } else {
                break;
            }
        } else {
            ctx.errors.push(Error::MissingCharacters {
                xpath: ctx.xpath.clone(),
            });
            return None;
        };
    }

    Some(TypeFunctionPointer {
        proto: fnptr_defn,
        params,
    })
}

fn process_define_code(code: String, name_: String, defref: Vec<String>) -> TypeDefine {
    fn consume_whitespace(chars: &mut std::str::Chars, mut current: Option<char>) -> Option<char> {
        while let Some(c) = current {
            if !c.is_whitespace() {
                break;
            }
            current = chars.next();
        }
        current
    }

    let mut is_disabled = true;
    let mut replace = false;
    let mut parameters = Vec::new();
    let mut value_ = None;
    let mut c_expr_ = None;
    let mut comment_ = None;

    {
        enum State {
            Initial,
            LineComment,
            BlockComment,
            DefineName,
            DefineArgs,
            DefineExpression,
            DefineValue,
        }
        let mut state = State::Initial;
        let mut chars = code.chars();
        loop {
            match state {
                State::Initial => {
                    let mut current = chars.next();
                    current = consume_whitespace(&mut chars, current);

                    match current {
                        Some('/') => {
                            current = chars.next();
                            match current {
                                Some('/') => state = State::LineComment,
                                Some('*') => state = State::BlockComment,
                                Some(c) => panic!("Unexpected symbol {:?}", c),
                                None => panic!("Unexpected end of code."),
                            }
                        }

                        Some('#') => {
                            let text = chars.as_str();
                            let mut directive_len = 0;
                            while let Some(c) = chars.next() {
                                if c.is_whitespace() {
                                    break;
                                }
                                if 'a' <= c && c <= 'z' {
                                    directive_len += 1;
                                } else {
                                    panic!("Unexpected symbol in preprocessor directive: {:?}", c);
                                }
                            }

                            let directive = &text[..directive_len];
                            match directive {
                                "define" => state = State::DefineName,
                                _ => {
                                    // Different directive. Whole text treated as c expression and replace set to true.
                                    replace = true;
                                    is_disabled = false;
                                    break;
                                }
                            }
                        }

                        Some('s') => {
                            let expected = "truct ";

                            let text = chars.as_str();
                            if text.starts_with(expected) {
                                // mk:TODO Less hacky handling of define which is actually forward declaration.
                                replace = true;
                                break;
                            } else {
                                println!("Unexpected code segment {:?}", code);
                            }
                        }

                        Some(c) => panic!("Unexpected symbol {:?}", c),
                        None => panic!("Unexpected end of code."),
                    }
                }

                State::LineComment => {
                    let text = chars.as_str();
                    if let Some(idx) = text.find('\n') {
                        let comment = text[..idx].trim();
                        if comment_.is_none() {
                            comment_.replace(String::from(comment));
                        }
                        chars = text[idx + 1..].chars();
                        state = State::Initial;
                    } else {
                        if comment_.is_none() {
                            comment_.replace(String::from(text.trim()));
                        }

                        break;
                    }
                }

                State::BlockComment => {
                    let text = chars.as_str();
                    if let Some(idx) = text.find("*/") {
                        let comment = &text[..idx];
                        if comment_.is_none() {
                            comment_.replace(String::from(comment));
                        }
                        chars = text[idx + 2..].chars();
                        state = State::Initial;
                    } else {
                        panic!("Unterminated block comment {:?}", text);
                    }
                }

                State::DefineName => {
                    is_disabled = false;
                    let text = chars.as_str();
                    let mut current = chars.next();
                    let mut whitespace_len = 0;
                    while let Some(c) = current {
                        if !c.is_whitespace() {
                            break;
                        }
                        current = chars.next();
                        whitespace_len += 1;
                    }

                    let mut name_len = 0;
                    while let Some(c) = current {
                        if !crate::c::is_c_identifier_char(c) {
                            break;
                        }
                        name_len += 1;
                        current = chars.next();
                    }

                    let name = &text[whitespace_len..whitespace_len + name_len];
                    if name != name_ {
                        panic!("#define name mismatch. {:?} vs. {:?}", name, name_);
                    }

                    match current {
                        Some('(') => state = State::DefineArgs,
                        Some(c) => {
                            if c.is_whitespace() {
                                state = State::DefineValue;
                            } else {
                                panic!("Unexpected char after #define name: {:?}", c);
                            }
                        }
                        None => break,
                    }
                }

                State::DefineArgs => {
                    let mut text = chars.as_str();
                    let mut current = chars.next();
                    loop {
                        let mut whitespace_len = 0;
                        while let Some(c) = current {
                            if !c.is_whitespace() {
                                break;
                            }
                            whitespace_len += 1;
                            current = chars.next();
                        }

                        let mut name_len = 0;
                        while let Some(c) = current {
                            if !crate::c::is_c_identifier_char(c) {
                                break;
                            }
                            current = chars.next();
                            name_len += 1;
                        }
                        let name = &text[whitespace_len..whitespace_len + name_len];
                        parameters.push(String::from(name));

                        current = consume_whitespace(&mut chars, current);
                        match current {
                            Some(',') => {
                                text = chars.as_str();
                                current = chars.next();
                            }
                            Some(')') => {
                                chars.next();
                                break;
                            }
                            Some(c) => {
                                panic!("Unexpected character in #define argument list: {:?}", c)
                            }
                            None => {
                                panic!("End of text while in the middle of #define argument list.")
                            }
                        }
                    }
                    state = State::DefineExpression;
                }

                State::DefineExpression => {
                    c_expr_.replace(String::from(chars.as_str().trim()));
                    break;
                }

                State::DefineValue => {
                    let v = String::from(chars.as_str().trim());
                    if !defref.is_empty() {
                        c_expr_.replace(v);
                    } else {
                        value_.replace(v);
                    }
                    break;
                }
            }
        }
    }

    if replace {
        c_expr_.replace(code);
    }
    let value = if let Some(expression) = c_expr_ {
        if parameters.is_empty() {
            TypeDefineValue::Expression(expression)
        } else {
            TypeDefineValue::Function {
                params: parameters,
                expression,
            }
        }
    } else if let Some(value) = value_ {
        TypeDefineValue::Value(value)
    } else {
        TypeDefineValue::Empty
    };
    TypeDefine {
        name: name_,
        comment: comment_,
        defref,
        is_disabled,
        replace,
        value,
    }
}

fn parse_type<R: Read>(ctx: &mut ParseCtx<R>, attributes: Vec<XmlAttribute>) -> TypesChild {
    let mut api = None;
    let mut alias = None;
    let mut requires = None;
    let mut name = None;
    let mut category = None;
    let mut parent = None;
    let mut returnedonly = None;
    let mut structextends = None;
    let mut allowduplicate = None;
    let mut objtypeenum = None;
    let mut bitvalues = None;
    let mut comment = None;

    let mut code = String::new();
    let mut markup = Vec::new();
    let mut members = Vec::new();

    match_attributes! {ctx, a in attributes,
        "api"            => api            = Some(a.value),
        "alias"          => alias          = Some(a.value),
        "requires"       => requires       = Some(a.value),
        "name"           => name           = Some(a.value),
        "category"       => category       = Some(a.value),
        "parent"         => parent         = Some(a.value),
        "returnedonly"   => returnedonly   = Some(a.value),
        "structextends"  => structextends  = Some(a.value),
        "allowduplicate" => allowduplicate = Some(a.value),
        "objtypeenum"    => objtypeenum    = Some(a.value),
        "bitvalues"      => bitvalues      = Some(a.value),
        "comment"        => comment        = Some(a.value)
    }

    let fn_ptr_spec = if let Some("funcpointer") = category.as_deref() {
        parse_type_funcptr(ctx)
    } else {
        None
    };

    let has_members = matches!(category.as_deref(), Some("struct" | "union"));

    match_elements_combine_text! {ctx, attributes, code,
        "member" if has_members => {
            let mut len = None;
            let mut altlen = None;
            let mut externsync = None;
            let mut optional = None;
            let mut selector = None;
            let mut selection = None;
            let mut noautovalidity = None;
            let mut validextensionstructs = None;
            let mut values = None;
            let mut limittype = None;
            let mut objecttype = None;
            let mut code = String::new();
            let mut markup = Vec::new();
            match_attributes!{ctx, a in attributes,
                "len"                   => len                   = Some(a.value),
                "altlen"                => altlen                = Some(a.value),
                "externsync"            => externsync            = Some(a.value),
                "optional"              => optional              = Some(a.value),
                "selector"              => selector              = Some(a.value),
                "selection"             => selection             = Some(a.value),
                "noautovalidity"        => noautovalidity        = Some(a.value),
                "validextensionstructs" => validextensionstructs = Some(a.value),
                "values"                => values                = Some(a.value),
                "limittype"             => limittype             = Some(a.value),
                "objecttype"            => objecttype            = Some(a.value)
            }
            if let Some(definition) = parse_name_with_type(
                ctx, &mut code,
                len,
                altlen,
                externsync,
                optional,
                noautovalidity,
                objecttype,
                |ctx, _code, local_name, _| match local_name {
                "comment" => {
                    let text = parse_text_element(ctx);
                    markup.push(TypeMemberMarkup::Comment(text));
                    ControlFlow::Continue(())
                },
                _ => ControlFlow::Break(())
            }) {
            members.push(TypeMember::Definition(TypeMemberDefinition {
                selector,
                selection,
                validextensionstructs,
                values,
                limittype,
                code,
                definition,
                markup,
            }))
        }
        },
        "comment" if has_members => members.push(TypeMember::Comment(parse_text_element(ctx))),
        "name" => {
            let text = parse_text_element(ctx);
            code.push_str(&text);
            markup.push(TypeCodeMarkup::Name(text));
        },
        "type" => {
            let text = parse_text_element(ctx);
            code.push_str(&text);
            markup.push(TypeCodeMarkup::Type(text));
        },
        "apientry" => {
            let text = parse_text_element(ctx);
            code.push_str(&text);
            markup.push(TypeCodeMarkup::ApiEntry(text));
        }
    }

    let spec = match category.as_deref() {
        Some("include") => TypeSpec::Include {
            name: markup.iter().find_map(|m| match m {
                TypeCodeMarkup::Name(name) => Some(name.clone()),
                _ => None,
            }),
            quoted: !code.contains("<"),
        },
        Some("define") => {
            let name_ = name
                .clone()
                .or_else(|| {
                    markup.iter().find_map(|m| match m {
                        TypeCodeMarkup::Name(name) => Some(name.clone()),
                        _ => None,
                    })
                })
                .unwrap();
            let defref = markup
                .iter()
                .filter_map(|m| match m {
                    TypeCodeMarkup::Type(ty) => Some(ty.clone()),
                    _ => None,
                })
                .collect();
            TypeSpec::Define(process_define_code(code, name_, defref))
        }
        Some("basetype") => TypeSpec::Typedef {
            name: markup
                .iter()
                .find_map(|m| match m {
                    TypeCodeMarkup::Name(name) => Some(name.clone()),
                    _ => None,
                })
                .unwrap(),
            basetype: markup.iter().find_map(|m| match m {
                TypeCodeMarkup::Type(ty) => Some(ty.clone()),
                _ => None,
            }),
        },
        Some("bitmask") => {
            if name.is_some() || alias.is_some() {
                TypeSpec::Bitmask(None)
            } else {
                TypeSpec::Bitmask(Some(NameWithType {
                    name: markup
                        .iter()
                        .find_map(|m| match m {
                            TypeCodeMarkup::Name(name) => Some(name.clone()),
                            _ => None,
                        })
                        .unwrap(),
                    type_name: markup
                        .iter()
                        .find_map(|m| match m {
                            TypeCodeMarkup::Type(ty) => Some(ty.clone()),
                            _ => None,
                        })
                        .unwrap(),
                    pointer_kind: None,
                    is_struct: false,
                    bitfield_size: None,
                    array_shape: None,
                    dynamic_shape: None,
                    externsync: None,
                    optional: None,
                    noautovalidity: None,
                    objecttype: None,
                }))
            }
        }
        Some("handle") => {
            if name.is_some() || alias.is_some() {
                TypeSpec::None
            } else {
                TypeSpec::Handle(TypeHandle {
                    name: markup
                        .iter()
                        .find_map(|m| match m {
                            TypeCodeMarkup::Name(name) => Some(name.clone()),
                            _ => None,
                        })
                        .unwrap(),
                    handle_type: match markup
                        .iter()
                        .find_map(|m| match m {
                            TypeCodeMarkup::Type(val) => Some(val.as_str()),
                            _ => None,
                        })
                        .unwrap()
                    {
                        "VK_DEFINE_HANDLE" => HandleType::Dispatch,
                        "VK_DEFINE_NON_DISPATCHABLE_HANDLE" => HandleType::NoDispatch,
                        h => unreachable!("Unexpected handle type {:?}", h),
                    },
                })
            }
        }
        Some("enum") => TypeSpec::Enumeration,
        Some("funcpointer") => TypeSpec::FunctionPointer(fn_ptr_spec.unwrap()),
        Some("struct") => TypeSpec::Struct(members),
        Some("union") => TypeSpec::Union(members),
        None => TypeSpec::None,
        Some(c) => unreachable!("Unexpected category of type {:?}", c),
    };

    TypesChild::Type(Type {
        api,
        alias,
        requires,
        name,
        parent,
        returnedonly,
        structextends,
        allowduplicate,
        objtypeenum,
        bitvalues,
        comment,
        spec,
    })
}

fn parse_command<R: Read>(ctx: &mut ParseCtx<R>, attributes: Vec<XmlAttribute>) -> Option<Command> {
    let mut name = None;
    let mut alias = None;
    let mut queues = None;
    let mut successcodes = None;
    let mut errorcodes = None;
    let mut renderpass = None;
    let mut videocoding = None;
    let mut cmdbufferlevel = None;
    let mut pipeline = None;
    let mut comment = None;

    match_attributes! {ctx, a in attributes,
        "name" => name = Some(a.value),
        "alias" => alias = Some(a.value),
        "queues" => queues = Some(a.value),
        "successcodes" => successcodes = Some(a.value),
        "errorcodes" => errorcodes = Some(a.value),
        "renderpass" => renderpass = Some(a.value),
        "videocoding" => videocoding = Some(a.value),
        "cmdbufferlevel" => cmdbufferlevel = Some(a.value),
        "pipeline" => pipeline = Some(a.value),
        "comment" => comment = Some(a.value)
    }

    if let Some(alias) = alias {
        unwrap_attribute!(ctx, command, name);
        consume_current_element(ctx);
        Some(Command::Alias { alias, name })
    } else {
        let mut code = String::new();
        let mut proto = None;
        let mut params = Vec::new();
        let mut description = None;
        let mut implicitexternsyncparams = Vec::new();

        match_elements! {ctx, attributes,
            "proto" => {
                proto = parse_name_with_type(ctx, &mut code, None, None, None, None, None, None, |_, _, _, _| ControlFlow::Break(()));
                code.push('(');
            },

            "param" => {
                let mut len = None;
                let mut altlen = None;
                let mut externsync = None;
                let mut optional = None;
                let mut noautovalidity = None;
                let mut objecttype = None;
                let mut validstructs = None;

                match_attributes!{ctx, a in attributes,
                    "len"            => len            = Some(a.value),
                    "altlen"         => altlen         = Some(a.value),
                    "externsync"     => externsync     = Some(a.value),
                    "optional"       => optional       = Some(a.value),
                    "noautovalidity" => noautovalidity = Some(a.value),
                    "objecttype"     => objecttype     = Some(a.value),
                    "validstructs"   => validstructs   = Some(a.value),
                }

                let validstructs = validstructs.map_or(
                    Default::default(),
                    |structs| structs.split(',').map(|s| s.to_owned()).collect()
                );

                if !params.is_empty() {
                    code.push_str(", ");
                }
                if let Some(definition) = parse_name_with_type(ctx, &mut code,
                    len,
                    altlen,
                    externsync,
                    optional,
                    noautovalidity,
                    objecttype,

                    |_, _, _, _| ControlFlow::Break(())) {
                    params.push(CommandParam {
                        definition,
                        validstructs,
                    });
                }
            },

            "alias" => {
                match_attributes!{ctx, a in attributes,
                    "name" => alias = Some(a.value)
                }
                consume_current_element(ctx);
            },

            "description" => description = Some(parse_text_element(ctx)),
            "implicitexternsyncparams" => {
                match_elements!{ctx,
                    "param" => implicitexternsyncparams.push(parse_text_element(ctx))
                }
            }
        }
        code.push_str(");");

        let proto = if let Some(v) = proto {
            v
        } else {
            ctx.errors.push(Error::MissingElement {
                xpath: ctx.xpath.clone(),
                name: String::from("proto"),
            });
            return None;
        };

        Some(Command::Definition(CommandDefinition {
            queues,
            successcodes,
            errorcodes,
            renderpass,
            videocoding,
            cmdbufferlevel,
            pipeline,
            comment,
            proto,
            params,
            alias,
            description,
            implicitexternsyncparams,
            code,
        }))
    }
}

fn parse_enum<R: Read>(ctx: &mut ParseCtx<R>, attributes: Vec<XmlAttribute>) -> Option<Enum> {
    let mut name = None;
    let mut comment = None;
    let mut type_suffix = None;
    let mut api = None;
    let mut extends = None;
    let mut value = None;
    let mut bitpos = None;
    let mut extnumber = None;
    let mut offset = None;
    let mut positive = true;
    let mut protect = None;
    let mut alias = None;

    match_attributes! {ctx, a in attributes,
        "name" => name = Some(a.value),
        "comment" => comment = Some(a.value),
        "type" => type_suffix = Some(a.value),
        "api" => api = Some(a.value),
        "extends" => extends = Some(a.value),
        "value" => value = Some(a.value),
        "offset" => offset = Some(a.value),
        "dir" => {
            if a.value.as_str() == "-" {
                positive = false;
            } else {
                ctx.errors.push(Error::UnexpectedAttributeValue {
                    xpath: ctx.xpath.clone(),
                    name: String::from("dir"),
                    value: a.value
                });
            }
        },
        "bitpos" => bitpos = Some(a.value),
        "extnumber" => extnumber = Some(a.value),
        "protect" => protect = Some(a.value),
        "alias" => alias = Some(a.value)
    }

    unwrap_attribute!(ctx, enum, name);

    let mut count = 0;
    if offset.is_some() {
        count += 1;
    }
    if bitpos.is_some() {
        count += 1;
    }
    if value.is_some() {
        count += 1;
    }
    if alias.is_some() {
        count += 1;
    }
    if count > 1 {
        ctx.errors.push(Error::SchemaViolation {
            xpath: ctx.xpath.clone(),
            desc: format!(
                "Unable to determine correct specification of enum: offset={:?}, bitpos={:?}, value={:?}, alias={:?}",
                offset, bitpos, value, alias
            ),
        });
        consume_current_element(ctx);
        return None;
    }

    let spec = if let Some(alias) = alias {
        EnumSpec::Alias { alias, extends }
    } else if let Some(offset) = offset {
        let offset = match parse_integer(ctx, &offset) {
            Some(v) => v,
            None => {
                consume_current_element(ctx);
                return None;
            }
        };
        if let Some(extends) = extends {
            EnumSpec::Offset {
                offset,
                extends,
                extnumber: match extnumber {
                    Some(extnumber) => parse_integer(ctx, &extnumber),
                    None => None,
                },
                dir: positive,
            }
        } else {
            ctx.errors.push(Error::SchemaViolation {
                xpath: ctx.xpath.clone(),
                desc: String::from("Missing extends on enum with offset spec."),
            });
            consume_current_element(ctx);
            return None;
        }
    } else if let Some(bitpos) = bitpos {
        let bitpos = match parse_integer(ctx, &bitpos) {
            Some(v) => v,
            None => {
                consume_current_element(ctx);
                return None;
            }
        };
        EnumSpec::Bitpos { bitpos, extends }
    } else if let Some(value) = value {
        EnumSpec::Value { value, extends }
    } else {
        EnumSpec::None
    };

    consume_current_element(ctx);

    Some(Enum {
        name,
        comment,
        type_suffix,
        api,
        protect,
        spec,
    })
}

fn parse_enums_child_unused<R: Read>(
    ctx: &mut ParseCtx<R>,
    attributes: Vec<XmlAttribute>,
) -> Option<EnumsChild> {
    let mut start = None;
    let mut end = None;
    let mut vendor = None;
    let mut comment = None;
    match_attributes! {ctx, a in attributes,
        "start"   => start   = Some(a.value),
        "end"     => end     = Some(a.value),
        "vendor"  => vendor  = Some(a.value),
        "comment" => comment = Some(a.value)
    }
    consume_current_element(ctx);
    unwrap_attribute!(ctx, unused, start);
    let start = match parse_integer(ctx, &start) {
        Some(v) => v,
        None => return None,
    };
    let end = end.and_then(|val| parse_integer(ctx, &val));
    Some(EnumsChild::Unused(Unused {
        start,
        end,
        vendor,
        comment,
    }))
}

fn parse_feature<R: Read>(
    ctx: &mut ParseCtx<R>,
    attributes: Vec<XmlAttribute>,
) -> Option<RegistryChild> {
    let mut api = None;
    let mut name = None;
    let mut number = None;
    let mut protect = None;
    let mut comment = None;
    let mut children = Vec::new();

    match_attributes! {ctx, a in attributes,
        "api"     => api     = Some(a.value),
        "name"    => name    = Some(a.value),
        "number"  => number  = Some(a.value),
        "protect" => protect = Some(a.value),
        "comment" => comment = Some(a.value)
    }

    match_elements! {ctx, attributes,
        "require" => children.push(parse_extension_item_require(ctx, attributes)),
        "remove"  => children.push(parse_extension_item_remove(ctx, attributes))
    }

    unwrap_attribute!(ctx, feature, api);
    unwrap_attribute!(ctx, feature, name);
    unwrap_attribute!(ctx, feature, number);

    Some(RegistryChild::Feature(Feature {
        api,
        name,
        number,
        protect,
        comment,
        children,
    }))
}

fn parse_extensions<R: Read>(
    ctx: &mut ParseCtx<R>,
    attributes: Vec<XmlAttribute>,
) -> RegistryChild {
    let mut comment = None;
    let mut children = Vec::new();

    match_attributes! {ctx, a in attributes,
        "comment" => comment = Some(a.value)
    }

    match_elements! {ctx, attributes,
        "extension" => if let Some(v) = parse_extension(ctx, attributes) {
            children.push(v);
        }
    }

    RegistryChild::Extensions(Extensions { comment, children })
}

fn parse_extension<R: Read>(
    ctx: &mut ParseCtx<R>,
    attributes: Vec<XmlAttribute>,
) -> Option<Extension> {
    let mut name = None;
    let mut comment = None;
    let mut number = None;
    let mut protect = None;
    let mut platform = None;
    let mut author = None;
    let mut contact = None;
    let mut ext_type = None;
    let mut requires = None;
    let mut requires_core = None;
    let mut supported = None;
    let mut deprecatedby = None;
    let mut promotedto = None;
    let mut obsoletedby = None;
    let mut provisional = None;
    let mut specialuse = None;
    let mut sortorder = None;
    let mut children = Vec::new();

    match_attributes! {ctx, a in attributes,
        "name"         => name          = Some(a.value),
        "comment"      => comment       = Some(a.value),
        "number"       => number        = Some(a.value),
        "protect"      => protect       = Some(a.value),
        "platform"     => platform      = Some(a.value),
        "author"       => author        = Some(a.value),
        "contact"      => contact       = Some(a.value),
        "type"         => ext_type      = Some(a.value),
        "requires"     => requires      = Some(a.value),
        "requiresCore" => requires_core = Some(a.value),
        "supported"    => supported     = Some(a.value),
        "deprecatedby" => deprecatedby  = Some(a.value),
        "promotedto"   => promotedto    = Some(a.value),
        "provisional"  => provisional   = Some(a.value),
        "obsoletedby"  => obsoletedby   = Some(a.value),
        "specialuse"   => specialuse    = Some(a.value),
        "sortorder"    => sortorder     = Some(a.value)
    }

    let number = match number {
        Some(text) => parse_integer(ctx, &text),
        None => None,
    };

    let provisional = match provisional {
        Some(value) => {
            if value == "true" {
                true
            } else {
                ctx.errors.push(Error::SchemaViolation {
                    xpath: ctx.xpath.clone(),
                    desc: format!("Unexpected value of 'provisional' attribute: {}", value),
                });
                false
            }
        }
        None => false,
    };

    let sortorder = match sortorder {
        Some(text) => parse_integer(ctx, &text),
        None => None,
    };

    unwrap_attribute!(ctx, extension, name);

    match_elements! {ctx, attributes,
        "require" => children.push(parse_extension_item_require(ctx, attributes)),
        "remove" => children.push(parse_extension_item_remove(ctx, attributes))
    }

    Some(Extension {
        name,
        comment,
        number,
        protect,
        platform,
        author,
        contact,
        ext_type,
        requires,
        requires_core,
        supported,
        deprecatedby,
        promotedto,
        obsoletedby,
        provisional,
        specialuse,
        sortorder,
        children,
    })
}

fn parse_extension_item_require<R: Read>(
    ctx: &mut ParseCtx<R>,
    attributes: Vec<XmlAttribute>,
) -> ExtensionChild {
    let mut api = None;
    let mut profile = None;
    let mut extension = None;
    let mut feature = None;
    let mut comment = None;
    let mut items = Vec::new();

    match_attributes! {ctx, a in attributes,
        "api"       => api       = Some(a.value),
        "profile"   => profile   = Some(a.value),
        "extension" => extension = Some(a.value),
        "feature"   => feature   = Some(a.value),
        "comment"   => comment   = Some(a.value)
    }

    while let Some(Ok(e)) = ctx.events.next() {
        match e {
            XmlEvent::StartElement {
                name, attributes, ..
            } => {
                let name = name.local_name.as_str();
                ctx.push_element(name);
                if let Some(v) = parse_interface_item(ctx, name, attributes) {
                    items.push(v);
                }
            }
            XmlEvent::EndElement { .. } => {
                ctx.pop_element();
                break;
            }
            _ => {}
        }
    }

    ExtensionChild::Require {
        api,
        profile,
        extension,
        feature,
        comment,
        items,
    }
}

fn parse_extension_item_remove<R: Read>(
    ctx: &mut ParseCtx<R>,
    attributes: Vec<XmlAttribute>,
) -> ExtensionChild {
    let mut api = None;
    let mut profile = None;
    let mut comment = None;
    let mut items = Vec::new();

    match_attributes! {ctx, a in attributes,
        "api"     => api     = Some(a.value),
        "profile" => profile = Some(a.value),
        "comment" => comment = Some(a.value)
    }

    while let Some(Ok(e)) = ctx.events.next() {
        match e {
            XmlEvent::StartElement {
                name, attributes, ..
            } => {
                let name = name.local_name.as_str();
                ctx.push_element(name);
                if let Some(v) = parse_interface_item(ctx, name, attributes) {
                    items.push(v);
                }
            }
            XmlEvent::EndElement { .. } => {
                ctx.pop_element();
                break;
            }
            _ => {}
        }
    }

    ExtensionChild::Remove {
        api,
        profile,
        comment,
        items,
    }
}

fn parse_interface_item<R: Read>(
    ctx: &mut ParseCtx<R>,
    name: &str,
    attributes: Vec<XmlAttribute>,
) -> Option<InterfaceItem> {
    match name {
        "comment" => Some(InterfaceItem::Comment(parse_text_element(ctx))),
        "type" => {
            let mut name = None;
            let mut comment = None;
            match_attributes! {ctx, a in attributes,
                "name"    => name    = Some(a.value),
                "comment" => comment = Some(a.value)
            }
            unwrap_attribute!(ctx, type, name);
            consume_current_element(ctx);
            Some(InterfaceItem::Type { name, comment })
        }
        "enum" => parse_enum(ctx, attributes).map(|v| InterfaceItem::Enum(v)),
        "command" => {
            let mut name = None;
            let mut comment = None;
            match_attributes! {ctx, a in attributes,
                "name"    => name    = Some(a.value),
                "comment" => comment = Some(a.value)
            }
            unwrap_attribute!(ctx, type, name);
            consume_current_element(ctx);
            Some(InterfaceItem::Command { name, comment })
        }
        _ => {
            ctx.errors.push(Error::UnexpectedElement {
                xpath: ctx.xpath.clone(),
                name: String::from(name),
            });
            return None;
        }
    }
}

fn parse_formats<R: Read>(ctx: &mut ParseCtx<R>) -> RegistryChild {
    let mut children = Vec::new();

    match_elements! {ctx, attributes,
        "format" => if let Some(v) = parse_format(ctx, attributes) {
            children.push(v);
        }
    }

    RegistryChild::Formats(Formats {
        comment: None,
        children,
    })
}

#[allow(non_snake_case)]
fn parse_format<R: Read>(ctx: &mut ParseCtx<R>, attributes: Vec<XmlAttribute>) -> Option<Format> {
    let mut name = None;
    let mut class = None;
    let mut blockSize = None;
    let mut texelsPerBlock = None;
    let mut blockExtent = None;
    let mut packed = None;
    let mut compressed = None;
    let mut chroma = None;
    let mut children = Vec::new();

    match_attributes! {ctx, a in attributes,
        "name"           => name           = Some(a.value),
        "class"          => class          = Some(a.value),
        "blockSize"      => blockSize      = Some(a.value),
        "texelsPerBlock" => texelsPerBlock = Some(a.value),
        "blockExtent"    => blockExtent    = Some(a.value),
        "packed"         => packed         = Some(a.value),
        "compressed"     => compressed     = Some(a.value),
        "chroma"         => chroma         = Some(a.value)
    }

    unwrap_attribute!(ctx, extension, name);
    unwrap_attribute!(ctx, extension, class);
    unwrap_attribute!(ctx, extension, blockSize);
    unwrap_attribute!(ctx, extension, texelsPerBlock);

    match_elements! {ctx, attributes,
        "component"        => if let Some(v) = parse_format_component(ctx, attributes) { children.push(v); },
        "plane"            => if let Some(v) = parse_format_plane(ctx, attributes) { children.push(v); },
        "spirvimageformat" => if let Some(v) = parse_format_spirvimageformat(ctx, attributes) { children.push(v); }
    }

    let blockSize: Option<u8> = parse_int_attribute(ctx, blockSize, "blockSize");
    let texelsPerBlock: Option<u8> = parse_int_attribute(ctx, texelsPerBlock, "texelsPerBlock");
    let packed = packed.map(|v| -> Option<u8> { parse_int_attribute(ctx, v, "packed") });

    let blockSize = match blockSize {
        Some(v) => v,
        None => return None,
    };

    let texelsPerBlock = match texelsPerBlock {
        Some(v) => v,
        None => return None,
    };

    let packed = match packed {
        Some(Some(v)) => Some(v),
        Some(None) => return None, // Attribute present, but parse error occurred.
        None => None,
    };

    Some(Format {
        name,
        class,
        blockSize,
        texelsPerBlock,
        blockExtent,
        packed,
        compressed,
        chroma,
        children,
    })
}

#[allow(non_snake_case)]
fn parse_format_component<R: Read>(
    ctx: &mut ParseCtx<R>,
    attributes: Vec<XmlAttribute>,
) -> Option<FormatChild> {
    let mut name = None;
    let mut bits = None;
    let mut numericFormat = None;
    let mut planeIndex = None;

    match_attributes! {ctx, a in attributes,
        "name"          => name          = Some(a.value),
        "bits"          => bits          = Some(a.value),
        "numericFormat" => numericFormat = Some(a.value),
        "planeIndex"    => planeIndex    = Some(a.value)
    }

    unwrap_attribute!(ctx, extension, name);
    unwrap_attribute!(ctx, extension, bits);
    unwrap_attribute!(ctx, extension, numericFormat);

    consume_current_element(ctx);

    let planeIndex =
        planeIndex.map(|v| -> Option<u8> { parse_int_attribute(ctx, v, "planeIndex") });

    let planeIndex = match planeIndex {
        Some(Some(v)) => Some(v),
        Some(None) => return None, // Attribute present, but parse error occurred.
        None => None,
    };

    Some(FormatChild::Component {
        name,
        bits,
        numericFormat,
        planeIndex,
    })
}

#[allow(non_snake_case)]
fn parse_format_plane<R: Read>(
    ctx: &mut ParseCtx<R>,
    attributes: Vec<XmlAttribute>,
) -> Option<FormatChild> {
    let mut index = None;
    let mut widthDivisor = None;
    let mut heightDivisor = None;
    let mut compatible = None;

    match_attributes! {ctx, a in attributes,
        "index"         => index         = Some(a.value),
        "widthDivisor"  => widthDivisor  = Some(a.value),
        "heightDivisor" => heightDivisor = Some(a.value),
        "compatible"    => compatible    = Some(a.value)
    }

    unwrap_attribute!(ctx, extension, index);
    unwrap_attribute!(ctx, extension, widthDivisor);
    unwrap_attribute!(ctx, extension, heightDivisor);
    unwrap_attribute!(ctx, extension, compatible);

    consume_current_element(ctx);

    let index: Option<u8> = parse_int_attribute(ctx, index, "index");
    let widthDivisor: Option<u8> = parse_int_attribute(ctx, widthDivisor, "widthDivisor");
    let heightDivisor: Option<u8> = parse_int_attribute(ctx, heightDivisor, "heightDivisor");

    let index = match index {
        Some(v) => v,
        None => return None,
    };

    let widthDivisor = match widthDivisor {
        Some(v) => v,
        None => return None,
    };

    let heightDivisor = match heightDivisor {
        Some(v) => v,
        None => return None,
    };

    Some(FormatChild::Plane {
        index,
        widthDivisor,
        heightDivisor,
        compatible,
    })
}

fn parse_format_spirvimageformat<R: Read>(
    ctx: &mut ParseCtx<R>,
    attributes: Vec<XmlAttribute>,
) -> Option<FormatChild> {
    let mut name = None;

    match_attributes! {ctx, a in attributes,
        "name" => name = Some(a.value)
    }

    unwrap_attribute!(ctx, extension, name);

    consume_current_element(ctx);

    Some(FormatChild::SpirvImageFormat { name })
}

fn parse_spirvextensions<R: Read>(
    ctx: &mut ParseCtx<R>,
    attributes: Vec<XmlAttribute>,
) -> RegistryChild {
    let mut comment = None;
    let mut children = Vec::new();

    match_attributes! {ctx, a in attributes,
        "comment" => comment = Some(a.value)
    }

    match_elements! {ctx, attributes,
        "spirvextension" => if let Some(v) = parse_spirvextension(ctx, attributes) {
            children.push(v);
        }
    }

    RegistryChild::SpirvExtensions(SpirvExtensions { comment, children })
}

fn parse_spirvextension<R: Read>(
    ctx: &mut ParseCtx<R>,
    attributes: Vec<XmlAttribute>,
) -> Option<SpirvExtension> {
    let mut name = None;
    let mut enables = Vec::new();

    match_attributes! {ctx, a in attributes,
        "name" => name = Some(a.value)
    }

    match_elements! {ctx, attributes,
        "enable" => if let Some(v) = parse_enable(ctx, attributes) {
            enables.push(v);
        }
    }

    unwrap_attribute!(ctx, spirvextension, name);

    Some(SpirvExtension { name, enables })
}

fn parse_spirvcapabilities<R: Read>(
    ctx: &mut ParseCtx<R>,
    attributes: Vec<XmlAttribute>,
) -> RegistryChild {
    let mut comment = None;
    let mut children = Vec::new();

    match_attributes! {ctx, a in attributes,
        "comment" => comment = Some(a.value)
    }

    match_elements! {ctx, attributes,
        "spirvcapability" => if let Some(v) = parse_spirvcapability(ctx, attributes) {
            children.push(v);
        }
    }

    RegistryChild::SpirvCapabilities(SpirvCapabilities { comment, children })
}

fn parse_spirvcapability<R: Read>(
    ctx: &mut ParseCtx<R>,
    attributes: Vec<XmlAttribute>,
) -> Option<SpirvCapability> {
    let mut name = None;
    let mut enables = Vec::new();

    match_attributes! {ctx, a in attributes,
        "name" => name = Some(a.value)
    }

    match_elements! {ctx, attributes,
        "enable" => if let Some(v) = parse_enable(ctx, attributes) {
            enables.push(v);
        }
    }

    unwrap_attribute!(ctx, spirvcapability, name);

    Some(SpirvCapability { name, enables })
}

fn parse_enable<R: Read>(ctx: &mut ParseCtx<R>, attributes: Vec<XmlAttribute>) -> Option<Enable> {
    let mut version = None;
    let mut extension = None;
    let mut struct_ = None;
    let mut feature = None;
    let mut requires = None;
    let mut alias = None;
    let mut property = None;
    let mut member = None;
    let mut value = None;

    match_attributes! {ctx, a in attributes,
        "version" => version = Some(a.value),
        "extension" => extension = Some(a.value),
        "struct" => struct_ = Some(a.value),
        "feature" => feature = Some(a.value),
        "requires" => requires = Some(a.value),
        "alias" => alias = Some(a.value),
        "property" => property = Some(a.value),
        "member" => member = Some(a.value),
        "value" => value = Some(a.value)
    }

    consume_current_element(ctx);

    if let Some(version) = version {
        Some(Enable::Version(version))
    } else if let Some(extension) = extension {
        Some(Enable::Extension(extension))
    } else if let Some(struct_) = struct_ {
        unwrap_attribute!(ctx, enable, feature);
        Some(Enable::Feature(FeatureEnable {
            struct_,
            feature,
            requires,
            alias,
        }))
    } else if let Some(property) = property {
        unwrap_attribute!(ctx, enable, member);
        unwrap_attribute!(ctx, enable, value);
        Some(Enable::Property(PropertyEnable {
            property,
            member,
            value,
            requires,
        }))
    } else {
        unimplemented!();
    }
}

fn parse_integer<R: Read>(ctx: &mut ParseCtx<R>, text: &str) -> Option<i64> {
    let parse_res = if text.starts_with("0x") {
        i64::from_str_radix(text.split_at(2).1, 16)
    } else {
        i64::from_str_radix(text, 10)
    };

    if let Ok(v) = parse_res {
        Some(v)
    } else {
        ctx.errors.push(Error::SchemaViolation {
            xpath: ctx.xpath.clone(),
            desc: format!("Value '{}' is not valid base 10 or 16 integer.", text),
        });
        None
    }
}

fn parse_int_attribute<I: FromStr<Err = std::num::ParseIntError>, R: Read>(
    ctx: &mut ParseCtx<R>,
    text: String,
    attribute_name: &str,
) -> Option<I> {
    match I::from_str(&text) {
        Ok(v) => Some(v),
        Err(e) => {
            ctx.errors.push(Error::ParseIntError {
                xpath: xpath_attribute(&ctx.xpath, attribute_name),
                text: text,
                error: e,
            });
            None
        }
    }
}

fn consume_current_element<R: Read>(ctx: &mut ParseCtx<R>) {
    let mut depth = 1;
    while let Some(Ok(e)) = ctx.events.next() {
        match e {
            XmlEvent::StartElement { name, .. } => {
                ctx.push_element(name.local_name.as_str());
                depth += 1;
            }
            XmlEvent::EndElement { .. } => {
                depth -= 1;
                ctx.pop_element();
                if depth == 0 {
                    break;
                }
            }
            _ => (),
        }
    }
}

fn parse_text_element<R: Read>(ctx: &mut ParseCtx<R>) -> String {
    let mut result = String::new();
    let mut depth = 1;
    while let Some(Ok(e)) = ctx.events.next() {
        match e {
            XmlEvent::StartElement { name, .. } => {
                ctx.push_element(name.local_name.as_str());
                depth += 1;
            }
            XmlEvent::Characters(text) => result.push_str(&text),
            XmlEvent::EndElement { .. } => {
                depth -= 1;
                ctx.pop_element();
                if depth == 0 {
                    break;
                }
            }
            _ => (),
        }
    }
    result
}
