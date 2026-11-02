# inspector_protocol

一个编译期的检查器 schema crate：用 `#[derive(Inspector)]` 在编译期收集结构体字段（无运行时反射），
生成字段元数据与写回函数。**不依赖 Bevy，也不做渲染、ECS 存储访问或网络通信**。
原生 UI 或其他消费者直接使用同一套字段类型；运行时依赖只有 Serde 和本仓库的 derive。

## 快速上手

```rust
use inspector_protocol::Inspector;

#[derive(Inspector)]
struct MyConfig {
    /// 布尔值 -> 复选框
    pub enabled: bool,
    /// 滑杆（带范围），并开启 "Motion" 分组
    #[inspector_group("Motion")]
    #[inspector(slider(min = -2.0, max = 2.0))]
    pub scale: f32,
    /// 数字输入框
    pub count: u32,
    /// 颜色 -> 颜色选择器
    pub color: bevy::color::LinearRgba,
    /// 向量 -> 分量输入框（vec2 / vec3 / vec4）
    pub offset: Vec2,
}
```

## 原理

`#[derive(Inspector)]` 在编译期收集字段（无运行时反射），生成
`Inspector::inspector_fields()` 和 `Inspector::apply_field()`。trait 本身不要求 `Resource` 或
`Component`，也可以用于普通结构体。ECS 约束属于使用方适配器：Resource 适配器通过
`ResMut<T>` 写回；Entity 适配器通过实体 ID 和 `get_mut::<T>()` 写回可变组件。

### 支持的字段类型

| 类型 | 默认控件 | 可指定 |
| --- | --- | --- |
| `bool` | 复选框 | — |
| 整数 / 浮点 | 输入框 | `#[inspector(slider(min, max, step))]` |
| 带 `#[inspector(dropdown)]` 的枚举 | 下拉菜单 | 需 `#[derive(Variants)]` |
| `LinearRgba` | 颜色选择器 | — |
| `Vec2` / `Vec3` / `Vec4` | 分量输入框 | — |
| 其他（字符串、数组…） | 跳过 | — |

### 分组

`#[inspector_group("标题")]` 是一个**独立属性**，用于开启一个分组：该字段以及**其后所有字段**都属于
这个组，直到遇到下一个 `#[inspector_group(..)]`（第一个分组属性之前的字段不带组）。前端把同组字段
放在一个可折叠标题下（默认展开）。它与控件属性互不影响，可以同时写：

```rust
#[inspector_group("Soil")]
#[inspector(slider(min = 0.0, max = 1.0))]
pub grain: f32,   // 属于 "Soil"
pub color: LinearRgba,   // 也属于 "Soil"
```

## 按钮与执行上下文

`FieldInfo.buttons` 与 `Inspector::inspector_buttons()` 只返回 `InspectorButton` 元数据，
包含 `id`、`label`、`hint` 和 `confirm`，不持有函数指针。点击由消费者转成动作请求。
需要自动生成分发时，使用普通 Rust 上下文：

```rust
use inspector_protocol::{Inspector, InspectorActions};

struct Context { calls: usize }
fn reset(context: &mut Context) -> Result<String, String> {
    context.calls += 1;
    Ok("reset".into())
}

#[derive(Inspector)]
#[inspector(context = Context)]
#[inspector(button(label = "Reset", run = reset, confirm))]
struct Settings { enabled: bool }

let mut context = Context { calls: 0 };
Settings::invoke_action(&mut context, "reset").unwrap();
assert_eq!(context.calls, 1);
```

Bevy 使用者可将 `Context` 换成其自身依赖的 `bevy::prelude::World`；宏只是原样生成这个类型路径，
协议与 derive 不引入 Bevy。字段上的按钮同样参加分发；类型内重复 ID 在编译时报错，未知动作返回错误。
如果由宿主另行处理动作，可只写 `#[inspector(button(id = "save", label = "Save"))]`，不需要上下文。
