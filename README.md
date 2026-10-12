# inspector_protocol

一个编译期的检查器 schema crate：用 `#[derive(Inspector)]` 在编译期收集结构体字段（无运行时反射），
生成字段元数据与写回函数。它本身不做渲染、不联网——`inspector_protocol_server` 会把这些元数据转成
Just Inspector 的 JSON WebSocket 协议，由 HTML 前端（`just_inspector` 仓库）渲染成控件。

## 快速上手

```rust
use inspector_protocol::Inspector;

#[derive(Resource, Inspector)]
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
`Inspector::inspector_fields()` 和 `Inspector::apply_field()`。写回通过 `ResMut<T>` 完成，
因此 `T` 必须是 `Resource`（`Mutability = Mutable`）。

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

## 旧版 Bevy UI 面板

`inspector_protocol::panel` / `draw_inspector` 这套 Bevy UI 渲染层已经移除，控制面板改由
`inspector_protocol_server` + HTML 前端提供；下面这段历史说明仅作参考。

## 通用 Panel（已移除）

下面的 `inspector_protocol::panel` API 属于旧的 Bevy UI 渲染层，已经删除，仅作历史参考：

`inspector_protocol::panel` 提供可复用的 UI 面板，具备以下能力：

- **标题栏**（可拖动整个面板）；
- **折叠/展开**：标题栏左侧的 `+` / `-` 按钮（无背景色）；
- **拖拽**：按住标题栏拖动；
- **缩放**：右下角 `◢` 三角把手，拖动改变宽高；
- **吸附（停靠）**：`snap_panel` 将面板停靠到窗口边缘，窗口改变大小时自动跟随。

```rust
use inspector_protocol::panel::{spawn_panel, snap_panel, unsnap_panel, PanelOptions, SnapEdge};

let handles = spawn_panel(&mut commands, PanelOptions {
    title: "Grass".into(),
    width: 340.0,
    ..Default::default()
});

// 水平吸附到右侧；或 SnapEdge::Bottom 垂直吸附到下方。
snap_panel(&mut commands, handles.root, SnapEdge::Right, 12.0);
// 解除吸附。
unsnap_panel(&mut commands, handles.root);
```

`spawn_panel` 返回的 `PanelHandles` 包含 `root` / `header` / `body` / `title` /
`resize_handle` 等实体句柄；把 `body` 传给 `draw_inspector` 即可把字段列表放进面板。

这些交互系统由 `InspectorPlugin` 自动注册（内部使用 `PanelPlugin`）。

## Bevy 兼容版本

当前依赖 Bevy ECS 0.20；工具链通过 `rust-toolchain.toml` 固定 Rust 1.97.1。
`inspector_protocol_server` 与调用方应使用同一 Bevy minor 版本，避免 ECS 类型不兼容。
