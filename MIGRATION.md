# 从 Go 迁移到 Rust

本文记录 `bombless/port-golang-windows-ime`（Go 版 TSF 输入法）迁移到本仓库时：迁了什么、没迁什么、
每一处**有意**的行为差异、原样继承的问题，以及哪些结论已被验证、哪些还没有。

> 此前 `main` 上的另一版 Rust 实现保存在 `backup/main-before-clear-2026-10-17` 与
> `feat/tsf-com-server` 分支里，本次迁移没有改动它们。

## 1. 范围

**已迁移**：`tip/`（拼音引擎、候选模型、候选窗口、COM 对象、编辑会话与组合串处理、注册、日志与诊断）、
`cmd/registerhost`、`install-dll.ps1`、`uninstall.ps1`，以及它们对应的测试。

**没有迁移，原因如下**：

| Go 仓库里的东西 | 为什么不迁 |
|---|---|
| `third_party/walk`、`third_party/go-com` | 被 `windows` crate 取代：`#[implement]` 直接生成 vtable 与引用计数，候选窗口改用原生 Win32 绘制，不再需要 GUI 工具包。 |
| `closedtsf/`、`itf_text_input_processor_generated.go`、`native_vtable_probe.go`、`fixture_test.go`、`processor_com_fixture_test.go` | 手写 vtable 与 cgo 薄层的产物。Rust 里 ABI 由 crate 生成，剩下要证明的是"服务对 TSF 的应答正确"，对应测试见第 5 节。 |
| `ABI-AUDIT.md`、`COVERAGE.md`、`cmd/closure-extract`、`Extract-Minimal.ps1`、`Verify-Coverage.ps1` | 全是针对 go-win32api 生成绑定与 `syscall.NewCallback` 的审计/提取工具链，问题本身在 Rust 里不存在。 |
| `cmd/ime-gui`、`install-gui.ps1` | 开发者控制台（构建/跑测试/开日志），不是 TSF 本体；Go 版 `PLAN.md` 自己也把它的去留列为待定。需要的话可以另行迁移。 |
| `DEBUG-PLAN.md`、各种 `*.log` | 针对旧的 pipe/GUI 架构的排障笔记与过期日志。 |

## 2. 文件对照

| Go | Rust |
|---|---|
| `tip/strategy.go` | `src/engine.rs`（字典、候选、reducer）、`src/session.rs`（`runLocalStrategy`、`notifyCandidates`、`restoreCandidates`） |
| `tip/candidate_ui.go` | `src/candidate_ui.rs` |
| `tip/candidate_presenter.go`（Walk） | `src/win/presenter.rs`（原生 Win32/GDI）、`src/layout.rs`（纯函数的排版与定位） |
| `tip/composition.go` | `src/win/composition.rs` |
| `tip/main.go`：COM 对象、按键 sink、类工厂、导出 | `src/win/service.rs`、`src/win/dll.rs` |
| `tip/main.go`：注册（C 代码） | `src/win/registration.rs` |
| `tip/main.go`：日志、`guardedCall`、dump | `src/diag.rs` |
| `cmd/registerhost` | `src/bin/registerhost.rs` |

## 3. 设计上的变化

- **COM**：一个 `#[implement(ITfTextInputProcessorEx, ITfKeyEventSink)]` 对象代替 Go 的三个 COM 对象。
  `ITfTextInputProcessorEx` 继承 `ITfTextInputProcessor`，所以对两者 `QueryInterface` 返回同一个指针，和 C++ 一致。
  没有任何 C 代码，也没有全局注册表竞态问题。
- **引用计数**：Go 版靠手写 `comRelease`；这里所有 COM 指针都是带 `Drop` 的包装，组合串、上下文、挂起的光标 range 都随所有权释放。
  Go 版把 `ITfContext` 当裸指针存着（`compositionContext`）；这里存的是带引用计数的接口。
- **线程与重入**：类按 `ThreadingModel=Apartment` 注册，TSF 回调只会来自同一线程，所以状态用 `Cell/RefCell`。
  真正要防的是**重入**（应用在我们的编辑会话里又回调按键 sink），不是并发。Go 用互斥锁串行化，并小心地在 `SetSelection` 前释放；
  同线程重复加锁会直接把宿主卡死，所以这里改成"忙标志"：重入的按键回调返回"未吞键"并写日志。有测试覆盖。
- **回复类型**：Go 内部把引擎结果编码成 JSON 字符串，再用手写扫描器解析回来（`replyString`/`replyConsume`/`describeReply`/`unquoteJSONString`）。
  这是旧的 pipe 协议的遗留物，这里直接用 `Reply` 结构体。对应的"解析 JSON"测试随之失去对象；它们要保护的行为（见第 5 节）保留了。
- **字典懒加载**：Go 在包初始化时解析 6.5 万行字典，每个加载 DLL 的进程都付这笔钱；这里首次使用时才解析。
- **分词 DP**：Go 对每个偏移量遍历全部 39k 个编码做前缀匹配；这里按前缀长度（≤20）直接查表。同一偏移处匹配的编码本来就按长度升序，
  所以顺序与结果和 Go 完全一致（差分测试证明），但快得多（同样 10.7 万条输入，Go 约 5 分钟，Rust 约 2 秒，含约 25 ms 的字典加载）。
- **候选窗口**：Walk 标签换成自己绘制的 `WS_EX_NOACTIVATE | WS_EX_TOPMOST` 弹出窗口；TSF 回调只发布快照（保留"最新者胜出"的合并语义），一个专用线程负责窗口和消息循环。
  字体、边距按快照的 DPI 缩放，宽度按真实文本测量而不是每字符 14px 估算。
- **dump**：Rust 无法遍历其他线程的栈，没有 `runtime.Stack(all)`。dump 改为列出"正在执行的 TSF 回调（含已耗时）+ 最近完成的回调 + 当前线程栈"，
  卡在锁上的回调会以很大的耗时出现，用途等价。panic 路径会在 panic 发生处用 hook 记录回溯（`catch_unwind` 返回时栈已展开）。

## 4. 有意的行为差异

每一条都是我主动选择的，不是疏漏。想恢复 Go 的行为，改对应位置即可。

1. **新的 CLSID / Profile GUID / 名称**（`Rust Chinese Input Method`）。这样能与 Go 版并存，在同一台机器上对比。
   要做成"替换 Go 版"，把 `src/guids.rs` 与 `install-dll.ps1` 里的 GUID 改成 Go 版的即可。安装脚本的冲突检查列表里已包含 Go 版身份。
   同时去掉了 Go 版对 `0x0409`/`0x0411` 遗留 profile 的清理——Rust 版没有前身。
2. **Ctrl / Alt / Win 组合键一律放行给应用**（`session::is_shortcut`）。Go 版只放行 Win+Space，
   于是启用后 Ctrl+C、Ctrl+V、Alt+字母全被吞掉并当成拼音。此前那版 Rust 实现在真机上遇到并修过同一个问题。Shift 不算（Shift+字母是输入大写）。
3. **日志默认脱敏**。Go 版把组合串、提交文本、整份回复原样写进 `%TEMP%` 里的日志（本 TIP 声明了安全模式类别，也就是密码框里也会激活）。
   这里默认只记长度（`<2 chars>`），`PortRustWindowsIMELogText=1` 才记原文，与 Go 版 `PLAN.md` 自己对候选日志的约束一致。
   候选日志每行不再 `fsync`（诊断日志不需要掉电安全，且每个事件一次 fsync 很慢）。
4. **前缀兜底阶段的平局顺序确定**（按编码字母序）。Go 版遍历 map 再用不稳定排序，平局时每次运行顺序都不同。
5. **按键回调内 panic 时返回 `S_OK` + 未吞键**，而不是 `E_FAIL`。key sink 返回失败 HRESULT 是会导致文本服务被 TSF 丢弃的那种事，
   引擎里的一个 bug 应该只损失一次按键。其余回调（Activate/Deactivate/导出）仍返回 `E_FAIL`。
6. **候选窗口定位的三处修正**（`src/layout.rs`，均有单元测试）：
   宽度为 0 但有高度的插入点矩形视为有效（`GetTextExt` 对折叠 range 通常就返回这种矩形，Go 版把它当无效并退回到鼠标位置）；
   放不下时翻到光标**顶边**之上（Go 版绕光标底边翻转，会盖住正在输入的那行）；
   边界钳制用光标所在显示器的工作区而不是整个虚拟屏幕。
7. **`Activate` 幂等**（已 advise 时重复调用不再二次 advise）、**`LockServer` 计数是进程级**（每次 `DllGetClassObject` 都会产生新的类工厂，计数不能放在工厂里）、
   **注册失败时完整回滚**（Go 版只回滚 COM 键，不回滚 TSF profile）。后三项来自 `docs/tsf-com-migration.md`（旧 Rust 分支）里对 COM 生命周期的验收清单。
8. **`DllCanUnloadNow` 恒为 `S_FALSE`**（与 Go 一致，但理由不同）：候选窗口线程与 dump 监视线程运行在本模块的代码里，卸载会让它们踩空。
9. **安装脚本**：`uninstall.ps1` 原先调用不存在的 `install.ps1`，已改为 `install-dll.ps1`；`registerhost.exe` 被拷到 `install\`，卸载不再依赖 `target\` 还在。
   `regsvr32` 在 Go 版上会以 `0xC0000409` 崩溃（Go 运行时的问题），Rust 版理论上不再有此问题，但安装器仍沿用 `registerhost`，因为它直接报告原始 HRESULT。

10. **所有交给 TSF 的宽字符串都带 NUL 结尾**（`session::utf16_nul_terminated`）。TSF 的 `AddLanguageProfile` 读描述字符串时读到 NUL 才停，不管传了多长的 `cchDesc`：
    最初的 Rust 实现传了不带 NUL 的缓冲区，约每 8 次注册就有 1 次把缓冲区后面的堆垃圾一起存进了语言栏显示的名字（CI 里重复注册 15 次抓到 2 次，长度 27/28 而不是 25，尾部是随机码点）。
    Go 版没遇到，因为它传的都是 C 字符串字面量，后面恒为 NUL。`SetText` 的缓冲区和"清空文本"的零长度调用也按同样方式处理（空切片的指针是悬空的，现在指向一个 NUL）。
    CI 现在每次都重复注册 30 次来守住这一点。

## 5. 原样继承、**没有**修改的问题

- `Space` 与 `Enter` 都提交选中的候选，而不是 `Enter` 提交原始拼音。
- 一个引擎不处理的键如果被直接送到 `OnKeyDown`（TSF 允许跳过 `OnTestKeyDown`），`forget_composition` 会清掉文档里的组合串，
  但引擎里的 `ImeState` 保持不变，两者就此失步。正常经过 `OnTestKeyDown` 的路径不会触发。
- 没有翻页（最多 9 个候选）、没有鼠标选词、没有用户词典、没有简繁转换。

## 6. 测试对照与验证状态

| Go 测试 | Rust |
|---|---|
| `strategy_test.go`、`keyconsume_test.go`（引擎与 eat 决策部分） | `engine.rs`、`session.rs` 的单元测试 |
| `candidate_ui_test.go` | `candidate_ui.rs`、`session.rs` |
| `composition_test.go`（`utf16Units`、written 状态、restore） | `session.rs` |
| `diagnostics_test.go`（`guardedCall`、`guidText`） | `diag.rs`、`guids.rs` |
| `closedtsf/layout_test.go`（接口身份） | `src/win/tests.rs::interface_and_category_identities_match_the_sdk_values` |
| `processor_com_fixture_test.go`（QI、引用计数、E_NOINTERFACE、E_POINTER） | `src/win/tests.rs` |
| 解析 JSON 的测试（`replyConsume`/`replyString`/`unquoteJSONString`） | 无（解析器已不存在）；"没有可显示内容时必须隐藏旧列表"这一保护行为保留 |
| **（Go 版没有）** 编辑会话 / 组合串 | `src/win/fake_tsf.rs` + `tests.rs`：假文档、假 range、假 composition、假 context，按 Go 注释里记录的真实宿主行为（`SetText` 不会让 range 增长）建模，覆盖 "你nin" 回归、提交、取消、过期 composition、重入、拒绝的编辑会话、焦点切换、反激活 |

**已验证**（可复现）：

- `tools/differential/run.sh`：原版 Go 引擎对比 Rust 引擎，候选生成与按键状态机**逐条完全一致**
  （参考实现里唯一的随机平局顺序被一行补丁固定为字母序，见脚本头注释）。
- 上表所有测试在 CI 的 Ubuntu 与 Windows 上通过；Windows 上还有"注册 → `CoCreateInstance` → 反注册"的真实注册表与 COM 端到端检查。

**没有验证**：DLL 没有在任何真实应用里被 TSF 加载过。候选窗口的实际位置、DPI、多显示器、各类宿主（记事本、浏览器、Office、UWP、终端、密码框）下的行为，
以及 ctfmon 的交互，都还需要真机验收，清单见 [PLAN.md](PLAN.md)。
