# TODO: Port `port-golang-windows-ime` to Rust

目标：将 [`bombless/port-golang-windows-ime`](https://github.com/bombless/port-golang-windows-ime) 的 Windows TSF 拼音输入法实现迁移到本仓库，最终产出可在 Windows 上注册、加载和实际输入的 Rust 实现。当前仓库是迁移起点，不代表迁移已完成。

## 当前已有（需复核）

- [x] 建立 Rust crate 基本结构：`Cargo.toml`、`src/lib.rs`。
- [x] 添加初步输入状态机：`src/engine.rs`，包含按键/候选/快照类型、预测按键结果、基础组合输入与选择逻辑、少量内置示例词和单元测试。
- [x] 添加 Windows 模块骨架：`src/windows/mod.rs`、`src/windows/tsf.rs`。
- [x] 添加项目 README。
- [ ] 完整输入法尚未完成；新增 COM/TSF 代码仍须由 Windows CI 编译验证，并在真实 TSF 宿主验收。

## 本轮实际核查与改动（2026-10-17）

- [x] 读取开发分支 `feat/tsf-com-server` 上的 `TODO.md`、`README.md`、`Cargo.toml`、`src/lib.rs`、`src/engine.rs`、`src/windows/tsf.rs` 和 CI workflow；确认当前 TSF 仍是生命周期占位模型，尚无 COM 类工厂、DLL 导出、真实 TSF 接口或 edit session。
- [x] 从 Go 源仓库 `tip/main.go` 的 C 定义与 Go 声明交叉核对身份：CLSID `C2D4A9B1-7F63-4A85-9E20-4B6D8C1F3072`，语言 profile GUID `E731B5D4-2A90-4F68-AC13-7D5E9B2046F1`；Rust 常量与之匹配。Go 源码还明确使用 `ITfTextInputProcessor`、`ITfTextInputProcessorEx`、`ITfKeyEventSink`，注册线程模型为 `Apartment`。
- [x] 为两个 GUID 增加回归测试。
- [x] 修正 CI 的 push 分支过滤器，将 `feat/**` 加入，确保开发分支 push 也触发 workflow（此前只有 `main` 和 `fix/**`）。
- [x] GitHub Actions workflow run #23（提交 `73a5601343d1b4b178d41aa07fb8314b52bc26c6`）已完成且结论为 success；该 workflow 包含 Ubuntu 格式/测试/lint 和 Windows MSVC x64 build job。该结果证明 CI 命令通过，不等同于在真实 TSF 宿主中注册、激活或输入验证。
- [x] GitHub Actions run #55（提交 `43396db`）的 Linux fmt/test/clippy 和 Windows MSVC x64 fmt/test/clippy/release build 全部通过；该次验证覆盖 COM class factory、TIP 和 key sink 注册代码。
- [ ] 在其后进一步改为 edit sessions 未就绪时按键全部透传，并调整了停用/锁处理；最新提交 `4f1997b` 的 CI 正在重新运行，尚未确认结果。
- [ ] 当前 Windows agent 未安装 Cargo，无法在本地重跑。未进行 Windows 实机 TSF 宿主验收。

## P0 — 先建立正确的源代码对应关系

- [ ] 完整检查 Go 源仓库的目录树、README、`go.mod`、所有 TSF/COM 实现、输入状态机、候选 UI、词典与测试。
- [ ] 建立 Go 文件/类型/行为到 Rust 文件/模块的迁移映射，避免只根据 README 或局部文件猜测行为。
- [ ] 继续核对 COM 接口、注册信息、线程模型、激活流程及资源清理要求；不要臆造 GUID 或接口签名。
- [ ] 确认目标支持的 Windows 版本、CPU 架构及构建/注册方式，并在 README 记录。

## P0 — 实现真正可加载的 TSF COM server

- [x] 增加 `IClassFactory`、`DllGetClassObject`、`DllCanUnloadNow` 的第一版实现；通过 `#[implement]` 生成 COM vtable，并跟踪活动对象和 `LockServer` 计数（待 Windows CI 验证）。
- [ ] 补充 `DllRegisterServer` / `DllUnregisterServer`、注册表与 TSF profile 注册/注销导出。
- [x] 增加 `ITfTextInputProcessor`、`ITfKeyEventSink` 的第一版 windows-rs `#[implement]` 实现；在 edit sessions 尚未就绪前，键盘回调明确返回未消费，避免吞掉用户输入。
- [ ] 对 COM 接口实现做编译和 ABI 审核，完善 `ITfTextInputProcessorEx` 兼容性。
- [x] 激活时 `AdviseKeyEventSink`、停用时 `UnadviseKeyEventSink`；避免持有服务状态锁跨越可能重入的 TSF COM 调用，撤销失败时保留 manager/active 状态，允许再次尝试。
- [ ] 完善重复激活/并发调用策略，并在真实 TSF 宿主验证连接与撤销。
- [ ] 实现 `ITfKeyEventSink` 的按键测试与处理：测试阶段不修改输入状态；处理阶段才消费按键并推进状态。
- [ ] 当前按键 sink 尚未写入文档：实现 `ITfEditSession` 与 `ITfContext::RequestEditSession`，在有效文档锁内更新/提交/删除组合文本，并覆盖同步/异步请求及锁失败。
- [ ] 完成 TSF profile、语言/输入法 profile、CLSID、注册表键值、位数匹配、安装/卸载和注册/注销说明。
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
- [ ] 实现候选窗显示、隐藏、定位、选中项、翻页/候选选择、DPI 与多显示器行为（以源实现实际支持为准）。
- [ ] 核实窗口创建线程、消息循环、焦点、窗口销毁和 TSF 上下文的关系，避免 UI 线程与 COM 线程问题。
- [ ] 在真实 Windows TSF 宿主中验证候选窗，而非仅依赖模拟器或编译成功。

## P1 — 构建、CI 和发布

- [ ] 本地/CI 执行 `cargo fmt --check`、`cargo test`、`cargo clippy --all-targets --all-features -- -D warnings`，修复全部失败。
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
