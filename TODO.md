# TODO: Port `port-golang-windows-ime` to Rust

目标：将 [`bombless/port-golang-windows-ime`](https://github.com/bombless/port-golang-windows-ime) 的 Windows TSF 拼音输入法实现迁移到本仓库，最终产出可在 Windows 上注册、加载和实际输入的 Rust 实现。当前仓库是迁移起点，不代表迁移已完成。

## 当前已有（需复核）

- [x] 建立 Rust crate 基本结构：`Cargo.toml`、`src/lib.rs`。
- [x] 添加初步输入状态机：`src/engine.rs`，包含按键/候选/快照类型、预测按键结果、基础组合输入与选择逻辑、少量内置示例词和单元测试。
- [x] 添加 Windows 模块骨架：`src/windows/mod.rs`、`src/windows/tsf.rs`。
- [x] 添加项目 README。
- [ ] 完整输入法尚未完成；COM 基础代码已通过一次 Windows MSVC x64 CI build，但仍须验证最新 head，并在真实 TSF 宿主验收。

## 本轮实际核查与改动（2026-10-17）

- [x] 本轮开始时读取开发分支上的 `TODO.md`、`README.md`、`Cargo.toml`、Rust 源码和 CI workflow；当时 TSF 只有生命周期占位模型，没有 COM 类工厂、DLL 导出或真实 TSF 接口。此后已在本分支添加第一版 COM/TIP/key-sink 实现。
- [x] 从 Go 源仓库 `tip/main.go` 的 C 定义与 Go 声明交叉核对身份：CLSID `C2D4A9B1-7F63-4A85-9E20-4B6D8C1F3072`，语言 profile GUID `E731B5D4-2A90-4F68-AC13-7D5E9B2046F1`；Rust 常量与之匹配。Go 源码还明确使用 `ITfTextInputProcessor`、`ITfTextInputProcessorEx`、`ITfKeyEventSink`，注册线程模型为 `Apartment`。
- [x] 为两个 GUID 增加回归测试。
- [x] 修正 CI 的 push 分支过滤器，将 `feat/**` 加入，确保开发分支 push 也触发 workflow（此前只有 `main` 和 `fix/**`）。
- [x] GitHub Actions workflow run #23（提交 `73a5601343d1b4b178d41aa07fb8314b52bc26c6`）已完成且结论为 success；该 workflow 包含 Ubuntu 格式/测试/lint 和 Windows MSVC x64 build job。该结果证明 CI 命令通过，不等同于在真实 TSF 宿主中注册、激活或输入验证。
- [x] GitHub Actions run #55（提交 `43396db`）的 Linux fmt/test/clippy 和 Windows MSVC x64 fmt/test/clippy/release build 全部通过。
- [x] GitHub Actions run #69（提交 `c581bc4`）再次通过 Linux fmt/test/clippy 和 Windows MSVC x64 fmt/test/clippy/release build；该版本已包含 key-sink 全部透传、避免跨 TSF 调用持锁、恢复 DLL 导出等修正。
- [x] 提交 `d95d758` / `1a44c74` / `6136099` 将引擎按键预测和正式处理接入 key sink，并让 edit session 在 TSF edit cookie 下通过 `ITfContext::GetSelection` 与 `ITfRange::SetText` 写入组合文本；提交/取消后清除跟踪的组合范围。
- [x] 重新核查 PR #5 当前 head `a30cbbf821ee5b53c5ddb17664d3f828dfa70fed`：GitHub Actions run #97 的 Linux 与 Windows job 均在格式检查失败；后续提交修正了 rustfmt 差异并增加虚拟键映射测试。
- [x] GitHub Actions run #105：Linux fmt/test/clippy 通过；Windows job 在 `cargo test --all-targets --all-features` 编译 Windows 专属代码时失败。日志确认 windows-rs 0.58 的安全包装签名与原代码不符：`GetSelection` 接收 `&mut [TF_SELECTION]`，`SetText` 接收 `&[u16]`，`RequestEditSession` 返回 `Result<HRESULT>`。提交 `c8d73d9` 已按实际编译器诊断修正这三处调用。
- [x] run #108 的 Linux/Windows 两个 job 在 rustfmt 检查失败，差异只涉及 `src/windows/mod.rs` 的模块顺序和 `registration.rs` 格式；已分别提交格式修正（`0986b45`、`e15ce8e`）。
- [x] run #119 的 Linux fmt/test/clippy 通过；Windows 编译推进到 TSF edit-session 代码后，发现 `TF_SELECTION.range` 是 `ManuallyDrop<Option<ITfRange>>`，原取值方式会 move 出该字段，并有一个不必要的 `mut`。提交 `cd9b48c` 改为借用并 clone `ITfRange`，移除 `mut`。
- [x] run #139 的 Linux fmt/test/clippy 通过；Windows `cargo test` 通过（9 tests），但 Windows Clippy 发现 Arc lint、缺少 Safety 文档和测试模块后的函数定义；提交 `40c8757` / `76352b1` 已修复。
- [x] run #147（head `ddf1395`）Linux fmt/test/clippy 全部通过，Windows fmt/test/clippy 和 `cargo build --release --target x86_64-pc-windows-msvc` 全部通过。
- [ ] 以上仅为 CI 编译/自动化测试；尚未在 Windows 实机注册、激活 TSF 服务或验证真实文本编辑行为。

## P0 — 先建立正确的源代码对应关系

- [ ] 完整检查 Go 源仓库的目录树、README、`go.mod`、所有 TSF/COM 实现、输入状态机、候选 UI、词典与测试。
- [ ] 建立 Go 文件/类型/行为到 Rust 文件/模块的迁移映射，避免只根据 README 或局部文件猜测行为。
- [ ] 继续核对 COM 接口、注册信息、线程模型、激活流程及资源清理要求；不要臆造 GUID 或接口签名。
- [ ] 确认目标支持的 Windows 版本、CPU 架构及构建/注册方式，并在 README 记录。

## P0 — 实现真正可加载的 TSF COM server

- [x] 增加 `IClassFactory`、`DllGetClassObject`、`DllCanUnloadNow` 的第一版实现；通过 `#[implement]` 生成 COM vtable，并跟踪活动对象和 `LockServer` 计数（run #55/#69 Windows CI 通过）。
- [x] 新增 `DllRegisterServer` / `DllUnregisterServer` 注册/注销 CLSID、DLL 路径和 Apartment 线程模型，并通过 `ITfInputProcessorProfiles` / `ITfCategoryMgr` 注册 zh-CN profile 与六个源 Go 对应类别；注册失败时回滚已创建的 profile/category。run #147 的 Windows x64 build 通过；实机注册验收仍未完成。
- [x] 增加 `ITfTextInputProcessor`、`ITfKeyEventSink` 的第一版 windows-rs `#[implement]` 实现；key sink 现在用 `Engine::test_key` 做无副作用预测，并在 `OnKeyDown` 调用 `handle_key`。
- [ ] 对 COM 接口实现做编译和 ABI 审核，完善 `ITfTextInputProcessorEx` 兼容性。
- [x] 激活时 `AdviseKeyEventSink`、停用时 `UnadviseKeyEventSink`；避免持有服务状态锁跨越可能重入的 TSF COM 调用，撤销失败时保留 manager/active 状态，允许再次尝试。
- [ ] 完善重复激活/并发调用策略，并在真实 TSF 宿主验证连接与撤销。
- [x] 增加 `ITfEditSession` COM 对象，并在 `OnKeyDown` 中通过 `ITfContext::RequestEditSession(TF_ES_SYNC | TF_ES_READWRITE)` 请求 TSF 写锁；目前仅验证调用链设计，未声称已能编辑文本。
- [x] 接通 `ITfKeyEventSink` 的按键测试与处理：测试阶段只调用 `Engine::test_key`；处理阶段调用 `handle_key` 并按结果请求 TSF edit session。仍需增加 COM/TSF 级别测试，确认请求失败时引擎状态回滚。
- [x] 增加 `ITfEditSession` 与 `ITfContext::RequestEditSession` 同步写锁路径；`DoEditSession` 使用有效 edit cookie 获取选区并调用 `ITfRange::SetText`，更新组合文本，提交或取消时清理范围状态。仍需 Windows 编译核实、验证 range/selection 实际行为；已加入 edit-session 请求失败时的引擎状态回滚，但仍需测试验证。
- [x] 已实现 TSF profile/category 注册与注销的第一版；仍需 Windows 实机验证、注册表视图/位数检查，以及可重复安装/卸载脚本。
- [ ] 检查 panic 不得越过 FFI/COM 边界；处理空指针、无效参数、HRESULT 与所有 unsafe 代码的安全约束。
- [ ] 让 COM 对象、线程管理器、sink、窗口与资源在停用/卸载时可重复且安全地清理。

## P1 — 迁移并验证输入引擎

- [ ] 对照 Go 实现迁移完整拼音组合、光标/编辑行为、候选生成、候选选择和提交逻辑；移除仅用于演示的硬编码小词表。
- [ ] 迁移词典加载/转换、编码、词频与排序行为，记录 Go/Rust 行为差异；不要默默改变兼容性。
- [ ] 覆盖字母输入、Backspace、Escape、方向键、数字键 1–9、Space、Enter、非法输入、空候选、重复事件及边界条件。
- [ ] 保证每个服务实例的组合串、候选列表、选中索引和相关状态互相隔离。
- [ ] 设计纯 Rust 单元测试：特别验证 `OnTestKeyDown` 等预测路径不会变更状态，只有正式处理路径才修改状态。
- [ ] 添加与 Go 版本行为对照的测试用例；必要时将 Go 输出固定为 golden fixtures。

## P1 — 候选窗与 UI

- [ ] 阅读并迁移 Go 的 `tip/candidate_ui.go`、`tip/candidate_presenter.go` 及其调用方，确认 UI 生命周期和交互细节。
- [x] 增加第一版原生 Win32 候选 popup：显示拼音组合与前 9 个候选，标记当前选中项；无候选或 composition 结束时隐藏，并跟随鼠标位置作为临时锚点。
- [ ] 将候选窗定位改为 TSF caret/text extent，补齐 DPI、多显示器工作区边界、候选点击/翻页和 UI 线程生命周期；当前 popup 仅是可见性原型，不能视为完整候选 UI。
- [ ] 核实窗口创建线程、消息循环、焦点、窗口销毁和 TSF 上下文的关系，避免 UI 线程与 COM 线程问题。
- [ ] 在真实 Windows TSF 宿主中验证候选窗，而非仅依赖模拟器或编译成功。

## P1 — 构建、CI 和发布

- [x] 历史提交在 run #69 中通过 Linux fmt/test/clippy 和 Windows MSVC x64 fmt/test/clippy/release build；这不代表当前 head 已通过。
- [x] run #97 格式检查失败已修复；run #105 的 Linux 检查通过，但 Windows 编译揭示了三个 windows-rs 0.58 方法签名错误，已在 `c8d73d9` 修复。最新 head 的 Windows CI 结果待确认。
- [x] CI 配置包含 Ubuntu 格式/测试/lint job 和 Windows MSVC x64 build job；push 触发器已覆盖 `feat/**`。
- [ ] Windows CI 构建通过后，核实生成物确实是可加载的 DLL，并核对导出符号、依赖项和目标架构。
- [ ] 增加最小安装/注册、卸载/注销脚本或明确步骤；脚本需可重复执行，并谨慎处理管理员权限和注册表。
- [ ] 更新 README：功能、限制、构建、安装/卸载、调试、日志、常见故障及已验证环境。

## P2 — Windows 实机验收

- [ ] 在干净的 Windows 环境完成注册并确认系统能发现、激活该输入法。
- [ ] 在至少一个标准文本输入宿主（如记事本）中验证组合输入、候选、确认提交、取消、退格和方向键。
- [ ] 验证多次启停、切换输入法、切换焦点、多个应用/线程及卸载后无崩溃、残留 sink 或窗口。
- [ ] 验证 x64；若项目声明支持其他架构，也分别验证对应构建和注册。
- [ ] 将未能实测的行为明确标注为“未验证”，不可仅凭静态检查声称成功。

## 完成标准

- [ ] Rust 实现与 Go 源版本的关键行为有明确对应关系，主要输入路径与候选交互已迁移。
- [ ] COM/TSF 接口、注册与 DLL 生命周期真实可用，而不是占位结构体或模拟适配器。
- [ ] 自动化测试和 Windows 构建通过，且至少完成一次 Windows 实机注册、激活和实际输入验收。
- [ ] 文档准确区分已实现、已测试、已在实机验证和仍存在的限制。

## TSF registration follow-up (2026-10-17)

- [x] Added `src/windows/tsf_registration.rs` to call `ITfInputProcessorProfiles::Register`, `AddLanguageProfile`, and `Unregister`, plus `ITfCategoryMgr::RegisterCategory` / `UnregisterCategory`, matching the Go `runRegistration` category GUIDs and zh-CN LANGID.
- [x] Wired profile registration after COM registry setup and profile/category removal before deleting the CLSID. On profile registration failure, attempt to remove the COM CLSID tree.
- [x] Updated PR #5 and README to describe the actual implementation and clearly state that current-head Windows validation is pending.
- [ ] Review raw COM vtable method indices/signatures against Windows SDK headers and confirm with the Windows MSVC CI run.
- [ ] Add cleanup for categories already registered if a later category registration fails.
- [ ] Verify registration/unregistration idempotence, elevation/registry behavior, and actual profile visibility on Windows.
- [ ] Do not treat CI compilation as proof of real TSF activation or text insertion.


## Candidate popup first pass (2026-10-17)

- [x] Add `src/windows/candidate_ui.rs`: create a non-activating topmost native STATIC popup, display the current composition and up to nine candidates on separate lines, mark the selected candidate, size the popup based on its content, and hide when the snapshot is not visible.
- [x] Publish immutable engine snapshots from the TSF key sink after successful handling, including selection-only key updates.
- [x] Add the Win32 keyboard/mouse feature needed for fallback cursor anchoring.
- [x] Add an explicit popup hide helper for TSF service deactivation/cleanup callers.
- [x] Prefer the focused GUI caret (GetGUIThreadInfo + ClientToScreen) over the cursor when positioning the popup; retain cursor fallback for hosts without a reported caret.
- [ ] Replace the GUI-caret approximation with the exact TSF caret/text extent when available, clamp to the monitor work area, and verify per-monitor DPI.
- [ ] Move popup ownership to a dedicated UI thread with a message loop and explicit shutdown. Current first pass creates the popup on the TSF callback thread and uses a process-global HWND, so multiple service instances and host-specific reentrancy are not yet supported.
- [x] Call the popup hide helper during TSF deactivation; safe HWND destruction and full resource lifecycle still need implementation.
- [ ] Verify the Windows MSVC build and visually test candidate show/update/hide in a real TSF host. No real-host validation is claimed by this commit.
- [ ] Candidate popup layout was updated to render candidates vertically with content-based dimensions; this remains compile/host-test pending.


## Candidate popup caret positioning follow-up (2026-10-17)

- [x] Candidate popup now anchors to the focused GUI caret rectangle converted to screen coordinates, following the Go presenter’s caret-first strategy; mouse position is only a fallback.
- [ ] Compare positioning behavior against the Go implementation in real TSF hosts, especially controls that do not expose a GUI caret.
- [ ] Clamp the popup to the active monitor work area and validate negative virtual-screen coordinates, multi-monitor layouts, and per-monitor DPI scaling.
- [ ] Verify Windows MSVC CI and visually test candidate placement in a real host; this change has not been built or visually validated in this session.
