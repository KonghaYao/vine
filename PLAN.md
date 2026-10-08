# PLAN: 鼠标操作 + 可拖拽滚动条（附 VSCode Git Graph 借鉴清单）

> 状态：**待评审**（本文件只描述方案，尚未开始实现）
>
> 范围决策（已确认）：
> 1. 计划文档放在仓库根 `PLAN.md`
> 2. 鼠标能力取 **最小集**：点击选中行、滚轮滚动、滚动条拖拽 + 点击跳转
> 3. 滚动条 **只加在提交列表视图**（commit list），其它视图不改
> 4. 鼠标捕获 **默认开启**，通过配置可关闭
> 5. **滚动与选中解耦**：滚动（滚轮、滚动条、Ctrl-e/Ctrl-y）只滚动视口内容，**不改变选中的提交**；选区允许滚出视口，不做"贴边钳制"（滚到边缘时不改选相邻提交）。翻页键（Ctrl-f/b、Ctrl-d/u、PgUp/PgDn）与滚动无关，保持现状。
> 6. **内置 Git 写操作**（切换分支、merge、revert、cherry-pick、reset、tag/stash 动作、远程同步…，对齐 VSCode Git Graph 的能力）：入口为"动作菜单"（鼠标右键 / 键位），破坏性操作用确认框；`actions.enable = false` 可整体关闭回纯浏览形态。注意这与 upstream 的 Non-Goals（"完整 Git 客户端""复杂 TUI 交互"）冲突——本仓库是其 fork，可以偏离，但实现上保持解耦（见 §7-6）。

---

## 1. 现状调研（代码事实）

1. **事件循环**：后台线程 poll crossterm 事件，只把 `Key` / `Resize` 转成 `AppEvent`，其余（含 `Mouse`）直接丢弃（`src/event.rs:121-147`）。
2. **事件总线**：`AppEvent` 与主线程 `recv()` 循环；主循环节奏为 `prepare_render → draw → recv`（`src/event.rs:19-57`、`src/app.rs:169-322`）。
3. **终端状态**：`ratatui::init()/restore()` 只处理 raw mode 与备用屏，**未启用鼠标捕获**；`EventController::suspend()/resume()` 在挂起型 user command（如 `vim`）前后切换终端状态（`src/main.rs:199-227`、`src/event.rs:151-172`）。
4. **视图分发**：`View`（List/Detail/UserCommand/Refs/Help）各有 `handle_event` / `render` / `update_layout`（`src/view/views.rs:28-60`）。
5. **列表状态**：`CommitListState { selected, offset, total, height, scrolloff }`，已有 `scroll_down/up`、`scroll_*_height`、`scroll_down_page/half`、`select_visible_index` 等（`src/widget/commit_list.rs:187-465`）。
6. **列表渲染**：`CommitList`（`StatefulWidget`）按 `calc_cell_widths` 分列渲染；graph 列走终端图像协议（iTerm2/Kitty）（`src/widget/commit_list.rs:756-795`、`:1117`）。
7. **列表复用**：Detail / Refs / UserCommand 视图内部也渲染 `CommitList`（`src/view/detail.rs:125-140` 等）。
8. **布局**：视图区 = 终端高度 − 2 行状态栏；视图自身再切分（详情面板、refs 面板）（`src/app.rs:455-457`）。
9. **配置**：`Config { core, ui, graph, color, keybind }`；`ui.common` / `ui.list` / 顶层 `color` 主题；`config.schema.json` 手工维护（`src/config.rs:88-104`、`:277-365`、`config.schema.json`）。
10. **文档**：mdbook，`docs/src/{getting-started,configurations,keybindings,features,...}`（`docs/src/SUMMARY.md`）。
11. **结论**：全仓库目前没有任何鼠标相关代码；坐标命中、滚动条几何、拖拽状态都需要从零建。

---

## 2. 设计

### 2.1 事件管道

1. `AppEvent` 新增 `Mouse(crossterm::event::MouseEvent)`；`event.rs` 的事件线程把 `Event::Mouse` 转发出来（改 `src/event.rs:127-134` 的 `_ => {}` 分支）。
2. `App` 主循环新增分支：`AppEvent::Mouse(mouse) => { ... self.view.handle_mouse_event(mouse) }`（`src/app.rs:169-322`）。
   - 搜索输入态（`StatusLine::Input` / 列表 `SearchState::Searching`）忽略鼠标事件。
   - `NotificationError` 等一次性状态沿用键盘的处理节奏（可选：仅忽略）。
3. `View` 增加 `handle_mouse_event(&mut self, mouse: MouseEvent)`，转发到各视图；最小集里只有 `ListView` / 内含 `CommitList` 的视图真正响应。
4. **鼠标捕获开关**（新增到 `EventController`）：
   - `enable_mouse_capture()` / `disable_mouse_capture()`：crossterm `EnableMouseCapture` / `DisableMouseCapture`。
   - 调用点：`main.rs` 在 `ratatui::init()` 之后启用（受配置控制）、`ratatui::restore()` 之前禁用；`EventController::resume()` 启用、`suspend()` 禁用（挂起型 user command 必须把鼠标还给外部程序）。
   - **panic hook**：`ratatui::init()` 安装的 hook 只恢复 raw mode / 备用屏，不会关鼠标模式，需要在它之后追加一层 hook 发送 `DisableMouseCapture`，否则崩溃后终端残留鼠标模式。
5. 配置：`ui.common.mouse: bool`（默认 `true`）。关闭时既不启用捕获，也不处理鼠标事件；同步更新 `config.schema.json` 与 `docs/src/configurations/config-file-format.md`。

### 2.2 命中测试与布局

1. 视图需要知道列表区域：在 `update_layout()` / `render()` 时把 `list_area: Rect` 存进 `ListView`（其它视图同理，若后续扩展）。
2. 行 → 提交映射：`commit_index = state.offset + (mouse_row - list_area.y)`；行号越界忽略。
3. 最小集不区分列：鼠标 x 落在该行内即选中该 commit（graph/marker/subject/… 任意列）。
4. 滚动条矩形：`list_area` 的**最右 1 列**；列表内容区 = `area` 去掉这 1 列（滚动条开启时）。
5. crossterm 给的 `column/row` 就是单元格坐标，与 ratatui `Rect` 一致，无需额外换算。

### 2.3 滚动条（方块滚轮）

1. **渲染位置**：`CommitList::render` 内先 `Layout::horizontal([Min(0), Length(1)])` 切出内容区与滚动条区，`calc_cell_widths` 用内容区宽度（`src/widget/commit_list.rs:762-770`）。
2. **几何**（自实现，便于命中测试与反向映射，~30 行纯函数）：

   ```
   H = 列表高度, N = 提交总数, O = 当前 offset
   若 H == 0 或 N <= H  →  不渲染滚动条（全部可见），也不响应鼠标
   thumb_len   = clamp(round(H * H / N), 1, H)
   thumb_start = round(O * (H - thumb_len) / (N - H))
   反解：O = round(thumb_start * (N - H) / (H - thumb_len))
   ```

   - 渲染与拖拽共用同一组函数，并加 round-trip 单元测试，避免"拖了和画的不一致"。
   - 不用 `ratatui::widgets::Scrollbar`：其内部公式不公开反解，命中测试会绑死内部实现细节。
3. **外观**：thumb 用 `█`（方块），track 用 `│`；颜色加到 `ColorTheme`（`list_scrollbar_thumb_fg` / `list_scrollbar_track_fg`，默认可参考 `divider_fg`），同步 schema 与文档。
4. **作用范围**：给 `CommitList` 加一个开关（如 `CommitList::new(ctx).scrollbar(true)`），只有 `ListView` 打开；Detail / Refs / UserCommand 内嵌的列表保持原样。

### 2.4 鼠标行为（List 视图）

**总原则：滚动只滚动内容，滚动不改变选中。** 滚动类操作只改变视口起点 `offset`，选中的提交保持不变；选区允许滚出视口（滚出后不高亮任何行，滚回来高亮自然恢复）。

1. **滚轮上 / 下**：`offset ∓ 3`（一"格"3 行），钳制到 `[0, N - H]`；**不改变选中的提交**。
2. **左键按下滚动条 thumb**：进入拖拽，记录 grab offset（鼠标 y − thumb_start）；不改变选中的提交。
3. **左键按下滚动条 track**：thumb 跳到点击处并进入拖拽（点击跳转 + 顺滑接续）；不改变选中的提交。
4. **左键拖拽（Drag）**：`offset = 反解(y - grab_offset)`，钳制到 `[0, N - H]`；不改变选中的提交。
5. **左键抬起**：结束拖拽。
6. **左键单击列表行**：该行 commit 被选中（点击即选中），视口不动 —— 唯一会改变选中提交的鼠标操作。
7. **其它输入**（右/中键、Moved、横向滚轮）：忽略（右键菜单等留给后续阶段）。
8. **不做"贴边钳制"**：滚动条跳到列表顶部/底部、滚轮把选区推出视口时，都不会把选区钳到首/末行 —— 选区始终停留在原来那个提交上。
9. **事件节流**：`Drag` 事件可能很密集，每个事件只更新 `offset`，不触发额外 IO；渲染仍由主循环统一进行。

### 2.5 状态模型调整（前置改动：选区改为绝对索引）

1. **问题**：当前 `CommitListState.selected` 是"视口内行号"（相对），绝对索引靠 `current_selected_index() = offset + selected` 临时算出来。这个模型**无法表示"选中的提交在视口之外"**；因此在实现"滚动不改变选中"时，唯一能做的就只能是滚动到边缘后把选区贴到边行 —— 而那恰恰就是改选了一个相邻提交（即 2.4 要避免的行为）。所以必须先改模型。
2. **新模型**：
   - `selected: usize` 语义改为**提交的绝对索引**（`0..total`），允许落在视口之外。
   - `offset: usize` 不变（视口首行索引）。
   - 派生 `selected_row() -> Option<usize>`（`selected - offset`，且 `< height` 时才是 `Some(row)`）；渲染高亮改用它，为 `None` 时不高亮任何行（`src/widget/commit_list.rs:1000` 的 `i == state.selected`）。
   - `current_selected_index()` 直接返回 `selected`；搜索跳转等逻辑随之简化。
3. **操作分类**：
   - **纯滚动（不动选区）**：滚轮、滚动条拖拽 / 轨道点击、`scroll_up/down`（Ctrl-e/Ctrl-y）—— 只改 `offset`，`selected` 不变，允许滚出视口。
   - **导航（改选区 + 滚回可见）**：`j/k`、`J/K`、`Alt-j`、`g/G`、`n/N`、`H/M/L`、点击行 —— 改 `selected`，再按 `scrolloff` 把选区滚回视口内（复用现有 `select_visible_index`）。
   - **翻页（与滚动无关，保持现状）**：`Ctrl-f/b`、`Ctrl-d/u`、`PageUp/PageDown` —— 维持现有行为：滚动一页并把选区保持在视口内（新模型下需要重写实现，但可观察行为不变）；不纳入"纯滚动"，本次不改语义。
4. **其它规则**：
   - 视口边界：`offset ∈ [0, max(0, total - height)]`。
   - resize（`update_height`，每帧都会调用）：保留 `selected` 与 `offset`（`offset` 按新高度钳制）；若选区在 resize 前可见、resize 后不可见，则把选区滚回视口内，避免出现"看不见但仍处于选中状态"的静默状态。
   - 刷新（`ListRefreshViewContext`，`src/view/views.rs:191-218`）：改为保存 `(commit_hash, selected 绝对索引, offset, height)`，恢复按"绝对索引 + offset"还原；`restore_selected_row` 相应改为 `restore_selected_index`。
   - 选区在视口外的视觉提示：本阶段不做（不高亮即视觉上无选区）；如需提示见 §8-1。
5. **影响面**：
   - `src/widget/commit_list.rs`：`selected` 的全部使用点（`grep` 计数：`self.selected` 26 处）+ 33 条依赖 `current_list_status()` 的既有测试断言需要按新语义重写。
   - `src/view/views.rs`：`ListRefreshViewContext` 及其 `From<&CommitListState>` 实现。
   - `src/view/list.rs`、`src/app.rs`：刷新恢复路径（`reset_commit_list_with`）。
6. **行为变更（需在文档/help 中说明）**：
   - `Ctrl-e` / `Ctrl-y` 从"高亮行钉在屏幕上、到边缘时改选相邻提交"变为"高亮随内容一起滚动、可滚出视口"。

### 2.6 新增状态

1. `ListView`：`list_area: Rect`、`dragging: Option<ScrollbarDrag>`（`{ grab_offset: u16 }`）。
2. `CommitListState`：新增 `set_offset(usize)`（纯滚动用，含钳制）、`selected_row() -> Option<usize>`、`scrollbar_geometry() -> Option<ScrollbarGeometry>`、`offset_for_thumb_start(u16) -> usize` 等纯函数 + 单测。
3. 不引入全局/线程间状态，沿用现有 `AppEvent` 单向流。

---

## 3. 实施步骤（每步可独立验证）

1. **配置**：`ui.common.mouse`（默认 true）+ `config.schema.json` + 配置文档。
   - 验证：`cargo test`（config 单测）、schema 手测解析。
2. **事件管道**：`AppEvent::Mouse`、`EventController` 捕获开关（含 panic hook、suspend/resume）、主循环分发。
   - 验证：手动 —— 启动后终端进入鼠标模式；`vim` 型 user command 挂起期间鼠标正常。
3. **状态模型重构**：`selected` 改绝对索引 + 纯滚动/导航语义拆分 + resize/刷新规则（先做，让"滚动不改变选中"在纯键盘下就成立）。
   - 验证：单测（重写既有断言 + 新语义）；手动 —— Ctrl-e/Ctrl-y 只滚内容、选区可滚出视口、`j` 把选区带回。
4. **布局记录 + 单击选中行**。
   - 验证：手动 —— 点击任意行选中正确 commit（含 offset≠0、列表短于一屏的边界）。
5. **滚轮滚动**（3 行/格，不动选区）。
   - 验证：手动 —— 滚轮/触控板在 iTerm2 与 kitty 下方向与幅度正确，选中提交不变。
6. **滚动条渲染**（预留 1 列 + 主题色 + `N<=H` 不显示）。
   - 验证：手动 + 单测（几何 round-trip）。
7. **滚动条拖拽 + 点击跳转**（不动选区，无贴边钳制）。
   - 验证：手动 + 单测（拖拽映射、边界钳制、`selected` 不变）。
8. **文档**：`docs/src/keybindings/index.md` 或新增 "鼠标操作" 小节（含滚动不改变选中的说明与键位行为变更）；README 提及；`?` 帮助视图（可选）。
   - 验证：`mdbook build`（在 `docs/` 下，需本地安装 mdbook）。
9. **Git 动作（阶段 1）**：动作菜单框架（popup）+ 只读动作（Copy Hash、打开详情、运行 user command）+ `Create Branch…`（输入框）。
   - 验证：手动 —— 菜单开关、执行后图刷新且保留选中提交。
10. **Git 动作（阶段 2）**：分支动作（checkout / merge / rebase / rename / delete）+ 确认框 + ref 选择器。
    - 验证：手动 + 单测（命令参数、ref 解析）。
11. **Git 动作（阶段 3）**：提交动作（revert / cherry-pick / reset）+ tag / stash 动作。
    - 验证：手动 + 单测（命令拼装；破坏性动作必须经过确认框）。
12. **Git 动作（阶段 4）**：远程动作（fetch / pull / push）+ 后台执行与进度提示。
    - 验证：手动 —— 长操作不阻塞 UI、失败时状态栏显示 stderr 摘要。

每步（含每个 Git 动作阶段）完成后同步更新配置 schema、文档与 `assets/default-keybind.toml`，并跑 CI 同款检查（`.github/workflows/build.yml`）：

```
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

---

## 4. 测试计划

**单元测试**（`src/widget/commit_list.rs` 内已有 `mod tests`，直接扩展）

1. 坐标 → 索引映射：offset=0 / offset>0 / 最后一行 / 越界行（返回 None）。
2. 滚动条几何：`H=1..N`、`N<=H`（返回 None）、`N=H+1` 等组合下 thumb 长度与位置；`offset ↔ thumb_start` round-trip。
3. 拖拽：grab offset 计算、拖到顶/底、`N<=H` 时无操作。
4. **滚动不改变选中**（状态模型重构后的核心回归）：
   - 纯滚动（`scroll_up/down`、滚轮、拖拽、轨道点击）前后 `selected` 不变，哪怕选区被推出视口。
   - 选区在视口外时 `selected_row() == None`（不高亮任何行），滚回视口后行号恢复正确。
   - 导航（`j/k`、`g/G`、`n/N`、点击）把选区滚回视口并尊重 `scrolloff`。
   - `H/M/L` 相对视口选取，且在列表尾部（不足一屏）时不越界。
   - resize：`offset` 按新高度钳制；resize 前可见、resize 后不可见的选区会被滚回视口。
   - 刷新恢复：按绝对索引 + `offset` 还原（含 `scroll_to_top` 分支）。

**手动测试矩阵**

1. 终端：iTerm2、kitty（含 `kitty-unicode` 协议）、tmux（需 `set -g mouse on`）。
2. 场景：滚动时 graph 图像同步重绘（不闪烁/不错位）；鼠标选中后键盘继续操作；搜索态忽略鼠标；resize 后命中位置仍正确；挂起型 user command（`vim`）返回后鼠标仍可用；`N <= H` 不显示滚动条；配置 `mouse = false` 时终端原生框选复制正常。

---

## 5. 风险与注意事项

1. **破坏终端原生选择**：开启鼠标捕获后，用户框选复制需要按 `Shift`（多数终端）。这是默认行为变更，需在文档中说明，并提供 `ui.common.mouse = false`。
2. **崩溃残留**：不处理 panic hook 会把终端留在鼠标模式（回显乱码），必须与 `ratatui::init()` 的 hook 正确串联。
3. **suspend/resume 必须成对**，否则运行 `vim` 时鼠标被 TUI 吞掉。
4. **图像协议与滚动**：滚动时既有图像重绘路径（`flush_pending_graph_uploads`）不变，但要避免鼠标事件引发额外 IO。
5. **高频事件**：拖拽期间每事件只算 offset；不要在事件处理里做全量重绘/上传。
6. **退化情况**：空仓库（`N=0`）、高度 0、`N<=H`、搜索态、help 覆盖视图。
7. **行为变更（需要文档说明）**：`Ctrl-e/Ctrl-y` 不再钉住高亮行、不再在边缘改选相邻提交；选区可滚出视口，此时界面上没有"当前提交"的视觉指示（见 §8-1）。
8. **`scrolloff` 的适用范围收窄**：只影响"导航时把选区滚回视口"的边距，不影响纯滚动。
9. **Git 写操作的破坏性**：`reset --hard`、删除分支/标签、`stash drop` 等不可逆操作必须经过确认框（`actions.confirm_destructive` 默认 true）；默认分支上执行危险操作时要给出明确提示。
10. **脏工作区**：checkout / merge / rebase 在有未提交改动时会失败 —— 直接展示 git 的 stderr，不做自动 stash、不做冲突解决（明确不在范围内）。
11. **命令构造安全**：动作命令一律 `Command::new("git").args([...])` 逐个传参，不拼接 shell 字符串；分支名/标签名等用户输入只作为单个 argv（避免注入与转义问题）。
12. **长操作阻塞**：阶段 4 之前 fetch/pull/push 是同步执行，会短暂卡住 UI；要么先明确提示，要么直接放到后台线程做（推荐后者，与 §7-2-8 一致）。
13. **upstream 偏离成本**：Git 动作用独立新文件 + 局部改动实现，避免大范围重构，降低后续跟随 upstream 的合并成本。

---

## 6. 借鉴 VSCode Git Graph 的功能清单（roadmap）

参考：Git Graph 扩展 README 与其设置项（<https://github.com/mhutchie/vscode-git-graph>）。upstream 的 Goals 是"围绕提交图浏览"、Non-Goals 是"完整 Git 客户端 / 复杂 UI"；**但按决策 6，本 fork 已把 git 写操作纳入实现范围（见 §7）**，因此下表中"不内置"一类的判断需按 §7 的方案重新解读：写操作内置，但走"动作菜单 + 确认框"，仍不追求完整客户端（不做冲突解决、交互式 rebase、remotes 管理界面、PR 创建）。

优先级：P0 = 本次实现；P1 = 低成本且与本次强相关；P2 = 中等价值；P3 = 需要先讨论。

### A. 鼠标与交互

1. **点击选中 / 打开详情** —— Git Graph：单击 commit 打开 Commit Details View；serie：仅键盘（`Enter` 开详情）；建议：**P0 单击选中**，P2 双击打开详情。
2. **多选比较两个提交** —— Git Graph：CTRL/CMD+click 第二个 commit → Compare View；serie：无；建议：P2，配合 §7 的动作体系做"选中两个 commit 比较"（`git diff A B`）。
3. **右键上下文菜单** —— Git Graph：对 commit/branch/tag/stash 右键出 git 动作菜单；serie：只有全局 user command 键位；建议：**P1**，右键弹出"动作菜单"（内置 git 动作 + user command 列表），设计见 §7。
4. **Hover 提示** —— Git Graph：悬浮 commit 顶点显示 tooltip（HEAD 是否包含、哪些分支/标签/储藏包含它）；serie：无；建议：P2，hover 时在状态栏显示提交摘要 + 所属 refs（TUI 版 tooltip）。
5. **拖拽滚动条** —— Git Graph：VS Code 原生滚动条；serie：无（本次）；建议：**P0**。
6. **快捷键：跳到 HEAD** —— Git Graph：`Ctrl+H` 把视图居中到 HEAD；serie：仅启动参数 `--initial-selection head`；建议：P1，新增 `go_to_head` 事件 + 键位（纯键盘，成本低）。
7. **快捷键：上/下一个 stash** —— Git Graph：`Ctrl+S` / `Ctrl+Shift+S`；serie：无；建议：P1，配合已有 `Ref::Stash` 实现跳转。
8. **滚动条上打标记** —— Git Graph：VS Code overview ruler / 缩略图；serie：无；建议：P1/P2，搜索匹配、HEAD、选中提交在滚动条上用小标记标出。

### B. 搜索与过滤

1. **Find Widget** —— Git Graph：搜索 commit message/日期/作者/hash/分支/标签，高亮 + 匹配计数；serie：已有（目标字段/忽略大小写/模糊/`n`/`N`/状态栏计数）；建议：已覆盖。
2. **只显示匹配（过滤模式）** —— Git Graph："匹配项过滤"视图；serie：无（只高亮 + 跳转）；建议：P1 过滤模式切换。
3. **搜索范围** —— Git Graph：支持日期区间等；serie：字段级范围已有；建议：P2 日期范围。
4. **分支过滤下拉** —— Git Graph：Show All / 多选分支 / 自定义 glob 模式；serie：仅 `--primary-branch` 与全量 refs；建议：P2 运行时"只看当前分支"切换。
5. **提交排序** —— Git Graph：date/topo + `--first-parent`、reflog-only 等；serie：`--order chrono|topo`；建议：P3 `--first-parent`、reflog 开关。

### C. 提交详情与文件视图

1. **详情内容** —— Git Graph：完整 message、作者/提交者、父提交、标签/分支、**签名状态**、Markdown 子集、issue 链接；serie：基本信息 + 文件变更列表；建议：P2 签名状态（`git log --show-signature` / `%G?`）、P2 相对日期、P3 Markdown/emoji/issue 链接。
2. **文件变更交互** —— Git Graph：点击用 VS Code diff 打开；可打开当前版本、复制路径；serie：user command（用户自配 `git diff` 等）；建议：P2，详情页对选中文件运行 user command / 复制路径（契合现有机制）。
3. **与任意提交比较** —— Git Graph：选中两个提交比较文件变更；serie：无；建议：P3（见 A-2 多选）。
4. **Uncommitted Changes 伪节点** —— Git Graph：工作区未提交变更作为伪节点显示、可比较；serie：无（`git log --branches --remotes --tags`）；建议：P3，先讨论是否属于 Goals。
5. **Code Review 状态** —— Git Graph：已审阅文件加粗/持久化（90 天）；serie：无；建议：P3/暂缓。

### D. 图形显示与主题

1. **Reference labels** —— Git Graph：可合并 local/remote 同名分支标签、对齐方式可选；serie：refs 直接显示在 subject 前；建议：P2，合并 local+remote 同名标签（常见诉求）、对齐选项。
2. **Mute 样式** —— Git Graph：非 HEAD 祖先提交、merge commit 淡化；serie：无；建议：P2（纯渲染改动）。
3. **作者头像** —— Git Graph：拉取并缓存 avatar；serie：已用终端图像协议画 graph，具备图像能力；建议：P3，用图像协议画头像（依赖协议/缓存/网络，需讨论）。
4. **Emoji shortcode** —— Git Graph：常见短码/gitmoji 自动替换，可自定义映射；serie：无；建议：P2/P3。
5. **列显示/宽度** —— Git Graph：运行时列宽拖拽 + 列显示切换；serie：配置文件静态定义 columns/宽度；建议：P3（鼠标拖拽列宽成本高，暂列 P3）。
6. **图形颜色/样式** —— Git Graph：可配置；serie：已有（`graph.color` / `--graph-style`）；建议：已覆盖。

### E. 大数据量体验

1. **分页加载** —— Git Graph：Initial Load + "Load More Commits" + 滚到底自动加载；serie：`-n/--max-count` 一次性加载；建议：P2，增量加载（需要改 `Repository::load` 与图计算为可增量，工作量较大）。
2. **Show tags/stashes/remotes 开关** —— Git Graph：设置项控制显示哪些 ref；serie：全量显示；建议：P3，配置开关过滤 refs。

### F. 集成与其它

1. **Fetch/Pull/Push、remotes 管理、PR 创建** —— Git Graph：内置按钮与设置面板；serie：原为 Non-Goal；建议：**改为内置（`fetch`/`pull`/`push` 按 §7-4 阶段 4 实现）**；remotes 管理界面与 PR 创建仍不做。
2. **复制内容** —— Git Graph：hash / subject / 分支/标签/储藏名 / 文件路径；serie：仅 commit hash（`c`/`C`）；建议：P1 复制 subject、ref 名（成本低），P2 文件路径（详情页）。
3. **自定义快捷键** —— Git Graph：VS Code 设置；serie：已支持（custom keybindings）；建议：已覆盖。
4. **配置随仓库共享** —— Git Graph：导出仓库级配置文件；serie：配置在 XDG（全局）；建议：P3，repo-local `.serie.toml` 覆盖（需讨论优先级与安全）。
5. **多仓库切换** —— Git Graph：仓库下拉；serie：单仓库（Non-Goal）；建议：不做。

---

## 7. Git 操作（写操作）设计

**目标**：像 VSCode Git Graph 一样，在界面上直接完成常用 git 操作（切换分支、merge、revert、cherry-pick、reset、tag/stash 动作、远程同步），不必切到终端。

**边界**：仍不做完整 Git 客户端 —— 不做冲突解决、不做交互式 rebase、不做 remotes 管理界面、不自动 fetch、不自动 stash。

### 7.1 动作清单（按目标分类）

**对提交（当前选中的 commit）**

1. Checkout（detached HEAD）：`git switch --detach <hash>`
2. Create Branch…（输入分支名）：`git switch -c <name> <hash>`
3. Create Tag…（输入标签名）：`git tag <name> <hash>`
4. Cherry-pick：`git cherry-pick <hash>`
5. Revert：`git revert --no-edit <hash>`
6. Reset（soft / mixed / hard 三选一；hard 需确认）：`git reset --soft|--mixed|--hard <hash>`
7. Copy Hash（已有能力，移入菜单）
8. 运行 user command（现有 user command 列表也放进菜单）
9. Compare with HEAD / 与另一提交比较（依赖多选，见 §6-A-2，P2）

**对本地分支**

1. Checkout / Switch：`git switch <branch>`
2. Merge into current：`git merge <branch>`
3. Rebase current onto：`git rebase <branch>`
4. Rename…：`git branch -m <old> <new>`
5. Delete（`git branch -d`；`-D` 强制删除需确认；当前分支/未合并分支给出明确报错）
6. Compare with current：`git diff <branch>..HEAD`
7. Push / Pull / Fetch（远程操作，见 §7-4 阶段 4）

**对远程分支**

1. Checkout（创建本地跟踪分支）：`git switch -c <local> --track <remote>/<branch>`
2. Merge into current：`git merge <remote>/<branch>`
3. Compare with current

**对标签**

1. Checkout（detached）：`git switch --detach <tag>`
2. Delete（需确认）：`git tag -d <tag>`
3. Push tag：`git push <remote> <tag>`（阶段 4）

**对 stash**

1. Apply：`git stash apply <stash>`
2. Pop：`git stash pop <stash>`
3. Drop（需确认）：`git stash drop <stash>`

### 7.2 交互设计

1. **动作菜单（popup）**：列出对当前上下文可用的动作；↑↓ 移动、Enter 执行、Esc 取消。
2. **入口**：鼠标右键（`MouseEventKind::Down(Right)`，与 §2.1 的鼠标管道衔接）；键盘新增 keybind 事件 `open_actions`（建议默认 `x`，可自定义）。
3. **上下文来源**：列表视图 = 当前选中的 commit（其挂载的 refs 作为二级菜单）；Refs 视图 = 当前选中的 ref（分支/标签/储藏）。
4. **目标选择器**：需要"另一个 ref"的动作（merge / rebase / compare）弹出可搜索的 ref 列表（复用 Refs 视图的树形列表 + 搜索）。
5. **输入框**：创建/重命名分支或标签时复用现有 `Input`（搜索框组件）。
6. **确认框**：破坏性动作（`reset --hard`、删除分支/标签、`stash drop`）弹 y/n 确认；`actions.confirm_destructive = false` 可关闭。
7. **结果反馈**：
   - 成功：状态栏显示一行摘要 + 刷新仓库（沿用现有 refresh 链路，保留选中提交）；有输出的动作进入输出视图（复用 `view/user_command.rs` 的 ANSI 渲染）。
   - 失败：状态栏 `NotificationError` 显示 git 的 stderr 摘要，不吞错。
8. **进度**：阶段 4 的远程操作放到后台线程执行 + 显示进度提示（需要新增 loading 状态）；在此之前同步执行并明确提示可能短暂卡顿。

### 7.3 实现要点

1. 新增 `src/git_action.rs`：`enum GitAction` + 每个动作 → `Vec<String>` 参数（纯函数，便于单测）；执行沿用 `git.rs` / `external.rs` 的 `Command::output()` 风格。
2. **命令构造一律用参数数组**（`Command::new("git").args([...])`），不做 shell 字符串拼接；用户输入（分支名、标签名）只作为单个 argv 传递。
3. 复用现有基础设施：刷新（`AppEvent::Refresh` 链路）、通知（`NotifyError` / `UpdateStatusTransient`）、输出渲染（`view/user_command.rs`）、输入组件（`Input`）、覆盖层渲染方式（help 视图的 popup 做法）。
4. 新增 widget：`ActionMenu`（popup 列表）、`ConfirmDialog`（y/n）、`RefPicker`（可搜索 ref 列表），放在 `src/widget/` 下并保持现有风格。
5. 配置新增（`config.rs` + `config.schema.json` + 文档）：`actions.enable: bool = true`（false 时动作菜单不可用，回到纯浏览形态）、`actions.confirm_destructive: bool = true`；keybind 新增 `open_actions`，菜单内的导航键（↑↓/Enter/Esc）固定不暴露。
6. 裸仓库保护：沿用 `git.rs:247` 的 `--is-bare-repository` 检查，裸仓库时禁用写操作。
7. 不自动 stash、不解决冲突：checkout / merge / rebase 失败时直接把 git 报错展示给用户。

### 7.4 分阶段

1. **阶段 1**：动作菜单框架 + 只读动作（Copy Hash、打开详情、运行 user command）+ 一个非破坏性写动作（Create Branch…）。
2. **阶段 2**：分支动作（checkout / merge / rebase / rename / delete）+ 确认框 + ref 选择器。
3. **阶段 3**：提交动作（revert / cherry-pick / reset）+ tag / stash 动作。
4. **阶段 4**：远程动作（fetch / pull / push）+ 后台执行与进度提示。

### 7.5 测试

1. 单测：每个动作 → 命令参数（覆盖带空格/特殊字符的分支名）、ref 解析（本地/远程/标签/stash）、破坏性动作必须经过确认框（用纯函数验证判定逻辑）。
2. 手动：在 demo 仓库走通 创建分支 → 切换 → merge → revert → reset（三种） → 删除分支；构造 merge 冲突，确认能看到 git 报错而不是崩溃。
3. 回归：`actions.enable = false` 时完全等同现状（无菜单、无写操作）。

### 7.6 与 upstream Non-Goals 的关系

1. upstream 明确把"完整 Git 客户端""复杂 TUI 交互"列为 Non-Goals（`docs/src/introduction/index.md`、`CONTRIBUTING.md:17`）；本仓库是 fork（`KonghaYao/serie`），偏离是自洽的。
2. 建议：动作体系由 `actions.enable` 统一切换 + 独立文件实现，保持与 upstream 解耦，降低后续合并成本。
3. 若将来要向上游提 PR，需先按 CONTRIBUTING 开 issue 讨论范围与预期行为。

---

## 8. 待确认问题（实现前）

**滚动 / 选中语义**

1. **选区滚出视口时是否需要视觉提示**：建议本阶段不做（不高亮即无选区）；后续可在滚动条上标记选区位置（与 §6-A-8 合并实现）。
2. **导航键的回归行为**：选区滚出视口后按 `j`/`k` 等，建议沿用 VS Code 树视图的做法 —— 从"选中的那个提交"继续移动，并把视图滚回选区（而不是从当前视口最近的行接手）。请确认可接受这种"视图跳回"。

**其它**

1. 滚轮每格滚动行数：建议 3 行（键盘 `scroll_down/up` 为 1 行）。
2. 点击滚动条 track：建议 thumb 直接跳到点击处并进入拖拽（不居中）。
3. 滚动条是否显示端点箭头：建议不显示，方块 thumb + 竖线 track，视觉最简。
4. 滚动条配色：默认沿用 `divider_fg` 一档的颜色，还是给主题加两个独立键（建议后者，可定制）。
5. 是否在 `?` 帮助视图与文档中列出鼠标操作（建议都列）。
6. `ui.common.mouse` 关闭时，是否仍保留"滚动条渲染"（仅不可拖拽）？（建议仍渲染，只是不响应鼠标）
