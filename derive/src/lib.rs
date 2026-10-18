//! `inspector_protocol` crate 的 proc-macro。
//!
//! - `#[derive(Inspector)]` 在编译期收集结构体的字段（没有反射），
//!   并为 [`inspector_protocol::Inspector`] 生成 `inspector_fields` / `apply_field`。
//! - `#[derive(Variants)]` 为一个 enum 生成下拉框元数据。
//!
//! 生成的代码通过 `inspector_protocol` 这个 crate 名去引用运行时，
//! 所以这个 derive 不依赖使用方 crate 的模块布局。

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    parse::ParseStream, parse_macro_input, spanned::Spanned, Data, DeriveInput, Fields, Ident, Lit,
    Token,
};

fn runtime_path() -> TokenStream2 {
    quote! { ::inspector_protocol }
}

// ---------------------------------------------------------------------------
// #[derive(Inspector)]
// ---------------------------------------------------------------------------

/// 一个字段映射到哪种控件（来自 `#[inspector(...)]` 属性，
/// 或按类型区分的默认控件）。
enum Control {
    Input,
    Slider {
        min: Option<f32>,
        max: Option<f32>,
        step: Option<f32>,
    },
    /// `slider(<path>)` —— 一个由用户提供的 `ControlSpec` const 表达式
    /// （preset），会原样粘贴进生成的元数据；宏从不检查它的值
    /// （见 `inspector_protocol` crate 的文档）。
    Spec(syn::ExprPath),
    Checkbox,
    Dropdown,
    /// `curve(value_min = .., value_max = .., time_min = .., time_max = ..)` ——
    /// 每个键都可选；缺失的键回退到 [`curve_ranges`] 里的默认值。
    Curve {
        value_min: Option<f32>,
        value_max: Option<f32>,
        time_min: Option<f32>,
        time_max: Option<f32>,
    },
}

/// 一个字段映射到的基础值类型。其他的一切都会被跳过。
enum Kind {
    Bool,
    Int,
    Float,
    Color,
    Enum,
    /// glam 的 `Vec2` / `Vec3` / `Vec4`，携带分量个数。
    Vector(u8),
    /// `inspector_protocol::Curve`。
    Curve,
    Skip,
}

#[proc_macro_derive(Inspector, attributes(inspector, inspector_group))]
pub fn derive_inspector(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand_inspector(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand_inspector(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let rt = runtime_path();

    let fields = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(named) => &named.named,
            other => {
                return Err(syn::Error::new(
                    other.span(),
                    "Inspector only supports structs with named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new(
                input.span(),
                "Inspector can only be derived for structs",
            ));
        }
    };

    let mut field_infos = Vec::new();
    let mut apply_arms = Vec::new();
    // 每个带按钮的字段一个 `const` 项，声明在 `inspector_fields` 的函数体里。
    let mut button_decls: Vec<TokenStream2> = Vec::new();
    let mut action_arms = Vec::new();
    let mut action_ids = std::collections::HashSet::new();

    // `#[inspector_group("X")]` 打开一个 section，其后每个字段都会加入它，
    // 直到下一个 group 属性，所以先把整列都解析出来。
    let groups = group_names(fields)?;

    for (field, group_name) in fields.iter().zip(&groups) {
        let Some(ident) = &field.ident else {
            continue;
        };
        let name_str = ident.to_string();
        let ty = &field.ty;
        let control = parse_field_attrs(field)?;
        // 挂在这个字段上的按钮（`#[inspector(button(..))]`），前端渲染在这一行之后。
        // 按钮表是一个 `const` 项（不能靠 `&[..]` 的常量提升：它在函数体里会被
        // 当成临时值）。
        let buttons = parse_buttons(&field.attrs, &rt, &format_ident!("BUTTONS_FOR_{ident}"))?;
        for (id, run) in &buttons.actions {
            if !action_ids.insert(id.value()) {
                return Err(syn::Error::new(id.span(), "duplicate inspector action id"));
            }
            if let Some(run) = run {
                action_arms.push(quote! { #id => #run(context), });
            }
        }
        if let Some(decl) = &buttons.decl {
            button_decls.push(decl.clone());
        }
        let buttons_expr = &buttons.expr;
        // 该字段 group 的 `Option<&'static str>` 表达式（或 `None`）。
        let group = match group_name {
            Some(group) => quote! { ::core::option::Option::Some(#group) },
            None => quote! { ::core::option::Option::None },
        };

        match classify(ty, &control) {
            Kind::Bool => {
                field_infos.push(quote! {
                    #rt::FieldInfo {
                        name: #name_str,
                        control: #rt::ControlSpec::Checkbox,
                        value: #rt::FieldValue::Bool(self.#ident),
                        options: &[],
                        unsigned: false,
                        step: ::core::option::Option::None,
                        group: #group,
                        buttons: #buttons_expr,
                    }
                });
                apply_arms.push(quote! { #name_str => self.#ident = value.as_bool(), });
            }
            Kind::Int => {
                let (control, step) = numeric_control(&rt, &control, 0.0, 100.0, 1.0);
                let unsigned = is_unsigned(ty);
                field_infos.push(quote! {
                    #rt::FieldInfo {
                        name: #name_str,
                        control: #control,
                        value: #rt::FieldValue::Int(self.#ident as i32),
                        options: &[],
                        unsigned: #unsigned,
                        step: #step,
                        group: #group,
                        buttons: #buttons_expr,
                    }
                });
                apply_arms.push(quote! { #name_str => self.#ident = value.as_int() as #ty, });
            }
            Kind::Float => {
                let (control, step) = numeric_control(&rt, &control, 0.0, 1.0, 0.1);
                field_infos.push(quote! {
                    #rt::FieldInfo {
                        name: #name_str,
                        control: #control,
                        value: #rt::FieldValue::Float(self.#ident as f32),
                        options: &[],
                        unsigned: false,
                        step: #step,
                        group: #group,
                        buttons: #buttons_expr,
                    }
                });
                apply_arms.push(quote! { #name_str => self.#ident = value.as_float() as #ty, });
            }
            Kind::Color => {
                field_infos.push(quote! {
                    #rt::FieldInfo {
                        name: #name_str,
                        control: #rt::ControlSpec::ColorPlane,
                        value: #rt::FieldValue::Color(self.#ident.to_f32_array()),
                        options: &[],
                        unsigned: false,
                        step: ::core::option::Option::None,
                        group: #group,
                        buttons: #buttons_expr,
                    }
                });
                apply_arms.push(quote! {
                    #name_str => self.#ident = <#ty>::from_f32_array(value.as_color()),
                });
            }
            Kind::Vector(components) => {
                let value_ctor = match components {
                    2 => quote! { #rt::FieldValue::Vec2(self.#ident.to_array()) },
                    3 => quote! { #rt::FieldValue::Vec3(self.#ident.to_array()) },
                    _ => quote! { #rt::FieldValue::Vec4(self.#ident.to_array()) },
                };
                let accessor = match components {
                    2 => quote! { value.as_vec2() },
                    3 => quote! { value.as_vec3() },
                    _ => quote! { value.as_vec4() },
                };
                field_infos.push(quote! {
                    #rt::FieldInfo {
                        name: #name_str,
                        control: #rt::ControlSpec::Vector,
                        value: #value_ctor,
                        options: &[],
                        unsigned: false,
                        step: ::core::option::Option::None,
                        group: #group,
                        buttons: #buttons_expr,
                    }
                });
                apply_arms.push(quote! {
                    #name_str => self.#ident = <#ty>::from_array(#accessor),
                });
            }
            Kind::Enum => {
                field_infos.push(quote! {
                    #rt::FieldInfo {
                        name: #name_str,
                        control: #rt::ControlSpec::Dropdown,
                        value: #rt::FieldValue::Enum(<#ty as #rt::Variants>::to_index(&self.#ident)),
                        options: <#ty as #rt::Variants>::variants(),
                        unsigned: false,
                        step: ::core::option::Option::None,
                        group: #group,
                        buttons: #buttons_expr,
                    }
                });
                apply_arms.push(quote! {
                    #name_str => self.#ident = <#ty as #rt::Variants>::from_index(value.as_index()),
                });
            }
            Kind::Curve => {
                let (value_min, value_max, time_min, time_max) = curve_ranges(&control);
                field_infos.push(quote! {
                    #rt::FieldInfo {
                        name: #name_str,
                        control: #rt::ControlSpec::Curve {
                            value_min: #value_min,
                            value_max: #value_max,
                            time_min: #time_min,
                            time_max: #time_max,
                        },
                        value: #rt::FieldValue::Curve(self.#ident.clone()),
                        options: &[],
                        unsigned: false,
                        step: ::core::option::Option::None,
                        group: #group,
                        buttons: #buttons_expr,
                    }
                });
                apply_arms.push(quote! { #name_str => self.#ident = value.as_curve(), });
            }
            Kind::Skip => {}
        }
    }

    // 挂在类型上的按钮（`#[inspector(button(..))]` 写在 struct 上）：前端渲染成
    // 面板顶部的一行。
    let struct_buttons = parse_buttons(&input.attrs, &rt, &format_ident!("BUTTONS_FOR_TYPE"))?;
    for (id, run) in &struct_buttons.actions {
        if !action_ids.insert(id.value()) {
            return Err(syn::Error::new(id.span(), "duplicate inspector action id"));
        }
        if let Some(run) = run {
            action_arms.push(quote! { #id => #run(context), });
        }
    }
    let context = parse_context(&input.attrs)?;
    let action_impl = if action_arms.is_empty() {
        None
    } else {
        let context = context.ok_or_else(|| {
            syn::Error::new(
                input.ident.span(),
                "buttons with run require #[inspector(context = YourContext)]",
            )
        })?;
        Some(quote! {
            impl #rt::InspectorActions<#context> for #name {
                fn invoke_action(context: &mut #context, action: &str) -> ::core::result::Result<::std::string::String, ::std::string::String> {
                    match action {
                        #(#action_arms)*
                        _ => ::core::result::Result::Err(::std::format!("Unknown inspector action {action}")),
                    }
                }
            }
        })
    };
    let struct_buttons_decl = struct_buttons.decl.iter();
    let struct_buttons_expr = &struct_buttons.expr;

    Ok(quote! {
        impl #rt::Inspector for #name {
            fn inspector_fields(&self) -> ::std::vec::Vec<#rt::FieldInfo> {
                #(#button_decls)*
                ::std::vec![ #(#field_infos),* ]
            }

            fn apply_field(&mut self, field: &str, value: #rt::FieldValue) {
                match field {
                    #(#apply_arms)*
                    _ => {}
                }
            }

            fn inspector_buttons(&self) -> &'static [#rt::InspectorButton] {
                #(#struct_buttons_decl)*
                #struct_buttons_expr
            }
        }
        #action_impl
    })
}

/// 为一个数值字段构造 `ControlSpec` 表达式以及 slider 的 `step`，
/// 并遵循可选的 `#[inspector(slider(...))]` 属性。
///
/// `dmin`/`dmax` 是属性省略 min/max 时使用的范围默认值；
/// `default_step` 是按类型区分的回退值，会被写进
/// `FieldInfo::step`（float 0.1，整数 1.0）。preset 路径
/// （`slider(RANGE_PERCENT)`）会原样作为控件表达式发出 ——
/// 宏无需知道 preset 的值 —— 并且同样拿到按类型区分的默认 step。
/// 把 `f32` 发成*符号分离*的表达式：`-256.0` 变成两个
/// token `- 256f32`，而不是一个有符号字面量。
///
/// `quote!` 的 `ToTokens for f32` 会产生单个负字面量 token
/// （`Literal::f32_suffixed(-256.0)`）；rustc 接受它，但 rust-analyzer 的
/// proc-macro token bridge 无法对有符号字面量分类，于是会在每个带负
/// `slider(min = ..)` / `max` / `step` 的 `#[derive(Inspector)]` 字段上报一个
/// 幽灵般的 `macro-error: expected Expr`。把符号拆开后，
/// 生成的代码含义不变，同时两个编译器都还能解析。
fn f32_expr(value: f32) -> TokenStream2 {
    if value.is_sign_negative() {
        let positive = -value;
        quote! { -#positive }
    } else {
        quote! { #value }
    }
}

fn numeric_control(
    rt: &TokenStream2,
    control: &Control,
    dmin: f32,
    dmax: f32,
    default_step: f32,
) -> (TokenStream2, TokenStream2) {
    match control {
        Control::Slider { min, max, step } => {
            let min = f32_expr(min.unwrap_or(dmin));
            let max = f32_expr(max.unwrap_or(dmax));
            let step = f32_expr(step.unwrap_or(default_step));
            (
                quote! { #rt::ControlSpec::Slider { min: #min, max: #max } },
                quote! { ::core::option::Option::Some(#step) },
            )
        }
        Control::Spec(expr) => (
            quote! { #expr },
            quote! { ::core::option::Option::Some(#default_step) },
        ),
        _ => (
            quote! { #rt::ControlSpec::Input },
            quote! { ::core::option::Option::None },
        ),
    }
}

fn classify(ty: &syn::Type, control: &Control) -> Kind {
    if matches!(control, Control::Dropdown) {
        return Kind::Enum;
    }
    if matches!(control, Control::Curve { .. }) {
        return Kind::Curve;
    }
    let syn::Type::Path(path) = ty else {
        return Kind::Skip;
    };
    let Some(segment) = path.path.segments.last() else {
        return Kind::Skip;
    };
    match segment.ident.to_string().as_str() {
        "bool" => Kind::Bool,
        "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize" => {
            Kind::Int
        }
        "f32" | "f64" => Kind::Float,
        "Vec2" => Kind::Vector(2),
        "Vec3" => Kind::Vector(3),
        "Vec4" => Kind::Vector(4),
        "LinearRgba" => Kind::Color,
        "Curve" => Kind::Curve,
        _ => Kind::Skip,
    }
}

/// `ControlSpec::Curve` 的范围表达式（以及属性省略某个键时
/// 它们各自的默认值）。
///
/// 数值轴默认取归一化信号用的带符号 `-1..1`
/// （一道裂口的边缘轮廓、一条 pan law……）；一条普通的振幅包络想要
/// `0..1`，可以这样写出来：`#[inspector(curve(value_min = 0.0))]`。
fn curve_ranges(control: &Control) -> (TokenStream2, TokenStream2, TokenStream2, TokenStream2) {
    let (value_min, value_max, time_min, time_max) = match control {
        Control::Curve {
            value_min,
            value_max,
            time_min,
            time_max,
        } => (*value_min, *value_max, *time_min, *time_max),
        _ => (None, None, None, None),
    };
    let f = |value: Option<f32>, default: f32| f32_expr(value.unwrap_or(default));
    (
        f(value_min, -1.0),
        f(value_max, 1.0),
        f(time_min, 0.0),
        f(time_max, 1.0),
    )
}

fn is_unsigned(ty: &syn::Type) -> bool {
    let syn::Type::Path(path) = ty else {
        return false;
    };
    let Some(segment) = path.path.segments.last() else {
        return false;
    };
    matches!(
        segment.ident.to_string().as_str(),
        "u8" | "u16" | "u32" | "u64" | "usize"
    )
}

/// `slider(...)` 收到了什么：要么是 `key = value` 键值对，
/// 要么是单条指向 [`ControlSpec`] preset const 的路径。
enum SliderBody {
    Keyed {
        min: Option<f32>,
        max: Option<f32>,
        step: Option<f32>,
    },
    Path(syn::ExprPath),
}

/// 读取一个字段上可选的 `#[inspector_group("Title")]` 属性。
///
/// 该属性会*打开*一个 section：被标注的字段及其后每个字段
/// 都属于它，直到下一个 `#[inspector_group(..)]`，所以整个字段列表是由
/// [`group_names`] 一次性解析的，而不是逐字段处理。
fn parse_group_attr(field: &syn::Field) -> syn::Result<Option<String>> {
    for attr in &field.attrs {
        if attr.path().is_ident("inspector_group") {
            let title: syn::LitStr = attr.parse_args()?;
            return Ok(Some(title.value()));
        }
    }
    Ok(None)
}

/// 解析每个字段所属的 group：在第一个
/// `#[inspector_group("X")]` 之前是 `None`，之后对该字段及其后所有字段
/// 都是 `Some("X")`，直到下一个 group 属性打开新的 section。
fn group_names(
    fields: &syn::punctuated::Punctuated<syn::Field, syn::Token![,]>,
) -> syn::Result<Vec<Option<String>>> {
    let mut current: Option<String> = None;
    let mut names = Vec::with_capacity(fields.len());
    for field in fields {
        if let Some(title) = parse_group_attr(field)? {
            current = Some(title);
        }
        names.push(current.clone());
    }
    Ok(names)
}

/// 解析 `field` 上每一个 `#[inspector(...)]` 属性并合并它们。
///
/// 每个属性都是一个逗号分隔的控件项列表（`slider(min = ..,
/// max = ..)`、`input`、`checkbox`、`dropdown`）；当多个属性设置同一个控件时，
/// 最后一个生效。没有任何属性的字段使用按类型区分的
/// 默认控件。
fn parse_field_attrs(field: &syn::Field) -> syn::Result<Control> {
    let mut control = Control::Input;
    for attr in &field.attrs {
        if !attr.path().is_ident("inspector") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            let Some(ident) = meta.path.get_ident() else {
                return Err(meta.error("expected an inspector control"));
            };
            match ident.to_string().as_str() {
                "input" => control = Control::Input,
                "checkbox" => control = Control::Checkbox,
                "dropdown" => control = Control::Dropdown,
                "slider" => {
                    control = if meta.input.peek(syn::token::Paren) {
                        let content;
                        syn::parenthesized!(content in meta.input);
                        let body = parse_slider_body(&content)?;
                        if !content.is_empty() {
                            return Err(content.error("unexpected tokens in slider(..)"));
                        }
                        match body {
                            SliderBody::Path(expr) => Control::Spec(expr),
                            SliderBody::Keyed { min, max, step } => {
                                Control::Slider { min, max, step }
                            }
                        }
                    } else {
                        Control::Slider {
                            min: None,
                            max: None,
                            step: None,
                        }
                    };
                }
                "curve" => {
                    control = if meta.input.peek(syn::token::Paren) {
                        let content;
                        syn::parenthesized!(content in meta.input);
                        let parsed = parse_curve_body(&content)?;
                        if !content.is_empty() {
                            return Err(content.error("unexpected tokens in curve(..)"));
                        }
                        parsed
                    } else {
                        Control::Curve {
                            value_min: None,
                            value_max: None,
                            time_min: None,
                            time_max: None,
                        }
                    };
                }
                // 按钮由 `parse_buttons` 单独解析（一个字段可以挂多个，而且它不改变
                // 控件），这里只要把它吃掉。
                "button" => {
                    let content;
                    syn::parenthesized!(content in meta.input);
                    let _: proc_macro2::TokenStream = content.parse()?;
                }
                other => {
                    return Err(meta.error(format!(
                        "unknown inspector control `{other}` (expected `slider`, `curve`, `input`, \
                         `checkbox`, `dropdown` or `button`)"
                    )));
                }
            }
            Ok(())
        })?;
    }
    Ok(control)
}

/// 解析 `slider(...)` 的内容。一条裸路径（`slider(RANGE_PERCENT)`，
/// 包括 `module::RANGE_PERCENT`）会被当作 preset const 表达式；
/// 其他任何东西都必须是 `min`/`max`/`step` 键值对。
fn parse_slider_body(content: ParseStream) -> syn::Result<SliderBody> {
    // 探测：整个参数列表能否解析成一条路径？能的话它就是 preset
    // （单独的 `RANGE_PERCENT` 也能解析成键值形式的第一个
    // token `min`，但那样还会剩下一个 `=`，所以探测在那里失败）。
    let fork = content.fork();
    if let Ok(path) = fork.parse::<syn::ExprPath>() {
        if fork.is_empty() {
            content.parse::<syn::ExprPath>()?;
            return Ok(SliderBody::Path(path));
        }
    }

    let mut min = None;
    let mut max = None;
    let mut step = None;
    while !content.is_empty() {
        let key: Ident = content.parse()?;
        content.parse::<Token![=]>()?;
        let value: Lit = content.parse()?;
        let value = match &value {
            Lit::Float(f) => f.base10_parse::<f32>().unwrap_or(0.0),
            Lit::Int(i) => i.base10_parse::<f32>().unwrap_or(0.0),
            _ => 0.0,
        };
        if key == "min" {
            min = Some(value);
        } else if key == "max" {
            max = Some(value);
        } else if key == "step" {
            step = Some(value);
        } else {
            return Err(syn::Error::new(
                key.span(),
                format!(
                    "unknown #[inspector(slider(..))] argument `{key}` (expected `min`, `max`, `step`, or a ControlSpec preset path)"
                ),
            ));
        }
        if content.peek(Token![,]) {
            content.parse::<Token![,]>()?;
        }
    }
    Ok(SliderBody::Keyed { min, max, step })
}

/// 解析 `curve(..)` 的内容：`value_min` / `value_max` / `time_min` /
/// `time_max` 键值对，全部可选（默认值在
/// [`curve_ranges`] 里）。
fn parse_curve_body(content: ParseStream) -> syn::Result<Control> {
    let (mut value_min, mut value_max, mut time_min, mut time_max) = (None, None, None, None);
    while !content.is_empty() {
        let key: Ident = content.parse()?;
        content.parse::<Token![=]>()?;
        let value: Lit = content.parse()?;
        let value = match &value {
            Lit::Float(f) => f.base10_parse::<f32>().unwrap_or(0.0),
            Lit::Int(i) => i.base10_parse::<f32>().unwrap_or(0.0),
            _ => 0.0,
        };
        match key.to_string().as_str() {
            "value_min" => value_min = Some(value),
            "value_max" => value_max = Some(value),
            "time_min" => time_min = Some(value),
            "time_max" => time_max = Some(value),
            other => {
                return Err(syn::Error::new(
                    key.span(),
                    format!(
                        "unknown #[inspector(curve(..))] argument `{other}` (expected \
                         `value_min`, `value_max`, `time_min` or `time_max`)"
                    ),
                ));
            }
        }
        if content.peek(Token![,]) {
            content.parse::<Token![,]>()?;
        }
    }
    Ok(Control::Curve {
        value_min,
        value_max,
        time_min,
        time_max,
    })
}

// ---------------------------------------------------------------------------
// #[derive(Variants)]
// ---------------------------------------------------------------------------

#[proc_macro_derive(Variants)]
pub fn derive_variants(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand_variants(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand_variants(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let rt = runtime_path();

    let Data::Enum(data) = &input.data else {
        return Err(syn::Error::new(
            input.span(),
            "Variants can only be derived for enums",
        ));
    };

    let mut names = Vec::new();
    let mut to_index_arms = Vec::new();
    let mut from_index_arms = Vec::new();

    for (index, variant) in data.variants.iter().enumerate() {
        let ident = &variant.ident;
        names.push(syn::LitStr::new(&ident.to_string(), ident.span()));
        to_index_arms.push(quote! { #name::#ident => #index });
        from_index_arms.push(quote! { #index => #name::#ident });
    }

    Ok(quote! {
        impl #rt::Variants for #name {
            fn variants() -> &'static [&'static str] {
                &[ #(#names),* ]
            }

            fn to_index(&self) -> usize {
                match self {
                    #(#to_index_arms),*
                }
            }

            fn from_index(index: usize) -> Self {
                match index {
                    // 末尾逗号是承重的：`_ => panic!(..)` 不是一个
                    // block arm，没有它两个 arm 会连在一起，
                    // 展开结果就成了语法错误。
                    #(#from_index_arms,)*
                    _ => panic!("invalid variant index for {}", ::core::stringify!(#name)),
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// #[inspector(button(..))]
// ---------------------------------------------------------------------------

/// 解析一个 `#[inspector(button(..))]` 按钮：
///
/// ```ignore
/// #[inspector(button(label = "生成 wave", run = inspector_wave))]
/// #[inspector(button(label = "清空", run = inspector_wave_clear, confirm))]
/// #[inspector(button(label = "发射", run = path::to::fire, hint = "…", id = "fire"))]
/// ```
///
/// - `label` 必填；`run` 可选，是 `fn(&mut Context) -> Result<String, String>` 的路径。
///   带 `run` 时类型必须声明 `#[inspector(context = Context)]`；
/// - `id` 缺省时取 `run` 路径的最后一段；不提供 `run` 时必须显式指定 `id`；
/// - `hint` 是 tooltip / 状态行；
/// - `confirm` 是裸标志，表示破坏性操作（前端先要一次确认）。
///
/// 返回值有两半：
///
/// - `decl`：一个 `const #name: &[InspectorButton] = &[..];` 项（没有按钮时是 `None`）。
///   之所以要显式声明 const，是因为 `&[..]` **不会**在函数体里被常量提升成
///   `&'static`（rustc 把它当作临时值，借用检查会报错）；
/// - `expr`：塞进 `FieldInfo::buttons` / `Inspector::inspector_buttons` 的表达式
///   （没有按钮时是 `&[]`）。
struct ButtonTable {
    decl: Option<TokenStream2>,
    expr: TokenStream2,
    actions: Vec<(syn::LitStr, Option<syn::Path>)>,
}

fn parse_context(attrs: &[syn::Attribute]) -> syn::Result<Option<syn::Type>> {
    let mut context = None;
    for attr in attrs
        .iter()
        .filter(|attr| attr.path().is_ident("inspector"))
    {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("context") {
                if context.is_some() {
                    return Err(meta.error("duplicate inspector context"));
                }
                context = Some(meta.value()?.parse()?);
            } else if meta.input.peek(syn::token::Paren) {
                let content;
                syn::parenthesized!(content in meta.input);
                let _: TokenStream2 = content.parse()?;
            } else {
                return Err(meta.error("expected context = Type or button(..)"));
            }
            Ok(())
        })?;
    }
    Ok(context)
}

fn parse_buttons(
    attrs: &[syn::Attribute],
    rt: &TokenStream2,
    const_name: &syn::Ident,
) -> syn::Result<ButtonTable> {
    let mut buttons = Vec::new();
    let mut actions = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("inspector") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if !meta.path.is_ident("button") {
                if meta.path.is_ident("context") {
                    let _: syn::Type = meta.value()?.parse()?;
                    return Ok(());
                }
                // 其他控件属性由 `parse_field_attrs` 负责；这里要跳过它的参数，
                // 否则 `parse_nested_meta` 会因为括号对不上而报错。
                if meta.input.peek(syn::token::Paren) {
                    let content;
                    syn::parenthesized!(content in meta.input);
                    let _: proc_macro2::TokenStream = content.parse()?;
                }
                return Ok(());
            }

            let content;
            syn::parenthesized!(content in meta.input);
            let mut label: Option<syn::LitStr> = None;
            let mut run: Option<syn::Path> = None;
            let mut id: Option<syn::LitStr> = None;
            let mut hint: Option<syn::LitStr> = None;
            let mut confirm = false;

            while !content.is_empty() {
                let key: syn::Ident = content.parse()?;
                match key.to_string().as_str() {
                    "confirm" => confirm = true,
                    "label" => {
                        content.parse::<syn::Token![=]>()?;
                        label = Some(content.parse()?);
                    }
                    "id" => {
                        content.parse::<syn::Token![=]>()?;
                        id = Some(content.parse()?);
                    }
                    "hint" => {
                        content.parse::<syn::Token![=]>()?;
                        hint = Some(content.parse()?);
                    }
                    "run" => {
                        content.parse::<syn::Token![=]>()?;
                        run = Some(content.parse()?);
                    }
                    other => {
                        return Err(syn::Error::new(
                            key.span(),
                            format!(
                                "unknown button key `{other}` (expected `label`, `run`, `id`, \
                                 `hint` or `confirm`)"
                            ),
                        ));
                    }
                }
                if content.peek(syn::Token![,]) {
                    content.parse::<syn::Token![,]>()?;
                }
            }

            let Some(label) = label else {
                return Err(syn::Error::new(
                    meta.path.span(),
                    "button(..) 需要 label = \"按钮文字\"",
                ));
            };
            // id 缺省 = 函数名（路径最后一段）—— 前端只回传它，所以同一个类型里要唯一。
            let id = match id {
                Some(id) => id,
                None => {
                    let run = run.as_ref().ok_or_else(|| {
                        syn::Error::new(
                            meta.path.span(),
                            "button(..) requires id when run is omitted",
                        )
                    })?;
                    let last = run
                        .segments
                        .last()
                        .map(|segment| segment.ident.to_string())
                        .unwrap_or_else(|| "button".to_string());
                    syn::LitStr::new(&last, run.segments.last().unwrap().ident.span())
                }
            };
            actions.push((id.clone(), run));
            let hint = match hint {
                Some(hint) => quote! { .hint(#hint) },
                None => quote! {},
            };
            let confirm = if confirm {
                quote! { .confirm() }
            } else {
                quote! {}
            };
            buttons.push(quote! {
                #rt::InspectorButton::new(#id, #label) #hint #confirm
            });
            Ok(())
        })?;
    }

    if buttons.is_empty() {
        return Ok(ButtonTable {
            decl: None,
            expr: quote! { &[] },
            actions,
        });
    }
    Ok(ButtonTable {
        decl: Some(quote! {
            const #const_name: &[#rt::InspectorButton] = &[ #(#buttons),* ];
        }),
        expr: quote! { #const_name },
        actions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_expansion_parses_with_context() {
        let input: DeriveInput = syn::parse_quote! {
            #[inspector(context = MyContext)]
            #[inspector(button(label = "Run", run = execute))]
            struct Settings { enabled: bool }
        };
        let expanded = expand_inspector(&input).unwrap();
        syn::parse2::<syn::File>(expanded).unwrap();
    }

    #[test]
    fn action_without_context_and_duplicate_ids_are_rejected() {
        let missing: DeriveInput = syn::parse_quote! {
            #[inspector(button(label = "Run", run = execute))]
            struct Settings { enabled: bool }
        };
        assert!(expand_inspector(&missing)
            .unwrap_err()
            .to_string()
            .contains("context"));
        let duplicate: DeriveInput = syn::parse_quote! {
            #[inspector(button(id = "same", label = "A"))]
            struct Settings {
                #[inspector(button(id = "same", label = "B"))]
                enabled: bool
            }
        };
        assert!(expand_inspector(&duplicate)
            .unwrap_err()
            .to_string()
            .contains("duplicate"));
    }

    #[test]
    fn context_is_not_implicitly_inferred_and_metadata_requires_id() {
        let missing: DeriveInput = syn::parse_quote! {
            #[inspector(button(label = "Run"))]
            struct Settings { enabled: bool }
        };
        assert!(expand_inspector(&missing)
            .unwrap_err()
            .to_string()
            .contains("id"));
        let duplicate: DeriveInput = syn::parse_quote! {
            #[inspector(context = First, context = Second)]
            struct Settings { enabled: bool }
        };
        assert!(expand_inspector(&duplicate).is_err());
    }

    /// `#[derive(Variants)]` 必须展开成真正能解析的代码。`from_index`
    /// 的 match 需要在生成的 arm 之后加一个末尾逗号，
    /// 因为后面跟的兜底 `_ => panic!(..)` 不是 block arm。
    #[test]
    fn variants_expansion_parses() {
        let input: DeriveInput = syn::parse_quote! {
            enum Tier {
                Quality,
                Performance,
            }
        };
        let expanded = expand_variants(&input).expect("expansion");
        syn::parse2::<syn::ItemImpl>(expanded).expect("expansion must be valid Rust");
    }

    /// 符号必须作为独立的 `-` token 保留下来：否则 `quote!` 的 `ToTokens for
    /// f32` 会把它折进字面量里，而 rust-analyzer 的
    /// proc-macro bridge 会以幽灵般的 `macro-error: expected Expr` 拒绝它。
    #[test]
    fn negative_floats_keep_the_sign_as_a_separate_token() {
        assert_eq!(f32_expr(-256.0).to_string(), "- 256f32");
        assert_eq!(f32_expr(-0.25).to_string(), "- 0.25f32");
        assert_eq!(f32_expr(0.5).to_string(), "0.5f32");
        assert_eq!(f32_expr(-0.0).to_string(), "- 0f32");
    }

    /// 一个控件属性可以解析（包括负的边界值），并且多个
    /// `#[inspector(..)]` 属性会合并。
    #[test]
    fn control_attributes_merge() {
        let field: syn::Field = syn::parse_quote! {
            #[inspector(slider(min = -1.0, max = 1.0, step = 0.01))]
            #[inspector(checkbox)]
            value: f32
        };
        match parse_field_attrs(&field).expect("attributes parse") {
            // 最后一个控件生效。
            Control::Checkbox => {}
            _ => panic!("expected the last control to win"),
        }

        let field: syn::Field = syn::parse_quote! {
            #[inspector(slider(min = -1.0, max = 1.0, step = 0.01))]
            value: f32
        };
        match parse_field_attrs(&field).expect("attributes parse") {
            Control::Slider { min, max, step } => {
                assert_eq!(min, Some(-1.0));
                assert_eq!(max, Some(1.0));
                assert_eq!(step, Some(0.01));
            }
            _ => panic!("expected a slider control"),
        }
    }

    /// 没有 `#[inspector(..)]` 的字段保持按类型区分的默认控件。
    #[test]
    fn no_control_attribute_is_the_default() {
        let field: syn::Field = syn::parse_quote! { center: Vec2 };
        assert!(matches!(
            parse_field_attrs(&field).expect("attributes parse"),
            Control::Input
        ));
    }

    /// `Curve` 字段按类型名分类，可选的
    /// `curve(..)` 属性会覆盖 widget 的轴范围。
    #[test]
    fn curve_fields_and_ranges() {
        let ty: syn::Type = syn::parse_quote! { Curve };
        assert!(matches!(classify(&ty, &Control::Input), Kind::Curve));

        let bare: syn::Field = syn::parse_quote! { edge: Curve };
        let (vmin, vmax, tmin, tmax) = curve_ranges(&parse_field_attrs(&bare).expect("parse"));
        assert_eq!(
            (vmin.to_string(), vmax.to_string()),
            ("- 1f32".to_string(), "1f32".to_string())
        );
        assert_eq!(
            (tmin.to_string(), tmax.to_string()),
            ("0f32".to_string(), "1f32".to_string())
        );

        let explicit: syn::Field = syn::parse_quote! {
            #[inspector(curve(value_min = 0.0, value_max = 2.0, time_min = -1.0, time_max = 3.0))]
            edge: Curve
        };
        let control = parse_field_attrs(&explicit).expect("parse");
        assert!(matches!(
            classify(&syn::parse_quote! { Curve }, &control),
            Kind::Curve
        ));
        let (vmin, vmax, tmin, tmax) = curve_ranges(&control);
        assert_eq!(
            (
                vmin.to_string(),
                vmax.to_string(),
                tmin.to_string(),
                tmax.to_string()
            ),
            (
                "0f32".to_string(),
                "2f32".to_string(),
                "- 1f32".to_string(),
                "3f32".to_string()
            )
        );
    }

    #[test]
    fn unknown_curve_argument_is_an_error() {
        let field: syn::Field = syn::parse_quote! {
            #[inspector(curve(nope = 1.0))]
            edge: Curve
        };
        assert!(parse_field_attrs(&field).is_err());
    }

    /// `#[inspector_group("..")]` 打开一个 section，其后每个字段
    /// 都会加入它，直到下一个 group 属性。
    #[test]
    fn groups_are_inherited_until_the_next_group_attribute() {
        let input: syn::DeriveInput = syn::parse_quote! {
            struct Cfg {
                lead: f32,
                #[inspector_group("A")]
                a1: f32,
                a2: f32,
                #[inspector(slider(min = 0.0, max = 1.0))]
                a3: f32,
                #[inspector_group("B")]
                b1: f32,
                b2: f32,
            }
        };
        let syn::Data::Struct(data) = &input.data else {
            panic!("expected a struct");
        };
        let syn::Fields::Named(named) = &data.fields else {
            panic!("expected named fields");
        };
        let groups = group_names(&named.named).expect("groups resolve");
        assert_eq!(
            groups,
            vec![
                None,
                Some("A".to_string()),
                Some("A".to_string()),
                Some("A".to_string()),
                Some("B".to_string()),
                Some("B".to_string()),
            ],
        );
    }

    #[test]
    fn inspector_group_without_a_title_is_an_error() {
        let field: syn::Field = syn::parse_quote! {
            #[inspector_group]
            value: f32
        };
        assert!(parse_group_attr(&field).is_err());
    }
}
