# IMPL — Windows 版本开发记录

基线：`dev/windows@0670f89`。本文只记录本次 Windows 开发中发现的文档偏差、未写明细节、技术坑与后续问题，不重复产品/协议契约。

## 1. 开发中发现文档的错误

### 1.1 ONNX Runtime 产物形态与文档不一致

**问题描述**
文档把 `onnxruntime.dll` 列为 Windows 发布必备产物，但 `ort 2.0.0-rc.12` 在当前 Windows/MSVC 预编译包中静态链接 ONNX Runtime，构建目录里没有可复制的 DLL。按文档验收会把正常的静态构建误判为缺文件。

**解决方法**
- 检查 `ort-sys` 构建输出，确认链接行为是 `cargo:rustc-link-lib=static=onnxruntime`。
- 安装脚本在找不到 DLL 时不再告警失败，改为说明静态链接。
- 文档改为“默认静态链接；只有切换到动态链接构建时才随包分发 `onnxruntime.dll`”。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `BUILD.html` | §2 构建根与命令；§7 发布 | 改 | 删除 Windows 必须有 `onnxruntime.dll` 的表述，补充分发形态取决于 `ort` 链接方式。 |
| `PLATFORM.html` | §6.6 发布产物 | 改 | Windows 产物写“`onnxruntime` 静态链接；动态链接构建时附带 DLL”。 |
| `WINDOWS.html` | §6.1 P0 #5；§7.3 验收门槛 | 改 | 验收重点改为 daemon 能加载模型并完成推理，不要求存在外部 DLL。 |
| `frontends/windows/BUILD.html` | §2 构建；§5 发布 | 改 | 产物表注明静态/动态两种形态，避免安装脚本误报。 |

### 1.2 实现状态与文件树已经过期

**问题描述**
README、PLATFORM 与 `frontends/windows` 子树仍写着 P3 未实现、`frontends/windows` 与 `packaging/windows` 待落地、源码树缺少 `settings.rs` 与 Windows 安装脚本。继续按旧状态阅读会误判构建根和验收范围。

**解决方法**
- 把 P3 状态改为已实现；TSF 明确保留为可选 P4，当前分支不包含。
- 根 CORE 与 Windows CORE 的源码树补齐 `settings.rs`、`packaging/windows` 与安装脚本。
- 根 README、PLATFORM、USER、voimed BUILD 的目标描述同步为实际状态。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `README.html` | 分支范围；§3 项目地图；§4 文档表 | 改 | P3 已实现，TSF 为可选 P4；项目地图与组件构建根同步。 |
| `PLATFORM.html` | 顶部状态；§3.1 目录树；§7 迁移阶段 | 改 | P2/P3 已落地；P4 的源码迁移已完成，TSF 子工程仍可选。 |
| `frontends/windows/README.html` | 状态 | 改 | 写明 P3 已实现、P4 未包含。 |
| `frontends/windows/CORE.html` | 状态；§1 文件树 | 改 | 状态改为已实现，文件树加入 `settings.rs`，并说明 `tsf/` 当前不存在。 |
| `frontends/windows/BUILD.html` | 状态；§3 安装 | 改 | 状态改为已实现，安装命令改为 PowerShell 脚本。 |
| `frontends/windows/USER.html` / `PROVIDER.html` | 状态 | 改 | 从“目标 P3/P4”改为“P3 已实现，P4 可选”。 |
| `CORE.html` | §1 根文件树；§4.7 | 改 | 增加 `packaging/windows`，Windows 前端状态改为 P3 已实现。 |

### 1.3 热键实现被描述成低级键盘钩子

**问题描述**
`frontends/windows/CORE` 把热键实现写成 `RegisterHotKey / 低级键盘钩子`。实际实现没有安装 `WH_KEYBOARD_LL`，而是用 `RegisterHotKey` 捕获按下，再用 `GetAsyncKeyState` 轮询 key-up；安全与验收描述因此与代码不一致。

**解决方法**
- 文件树改为 `RegisterHotKey / GetAsyncKeyState key-up`。
- 明确 PTT 的 key-up 由主循环轮询，不依赖低级钩子；Toggle 用按键状态去重，避免自动重复切换。
- `PROVIDER` 保留 `GetAsyncKeyState` 作为允许的 key-up 备选。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/CORE.html` | §1 文件树；§4.4 hotkey.rs | 改 | 删除“低级键盘钩子”的默认描述，写明 `RegisterHotKey + GetAsyncKeyState`。 |
| `frontends/windows/PROVIDER.html` | §4.1 Win32 / UIA 调用 | 改 | 说明 `WH_KEYBOARD_LL` 不是当前默认路径；`GetAsyncKeyState` 是当前 key-up 方案。 |
| `frontends/windows/USER.html` | §2 CONVENTIONS | 不动 | 行为只要求 PTT 有 key-up、不记录其他按键，具体实现由 PROVIDER/CORE 决定。 |

### 1.4 Windows 传输的阻塞与取消描述与实现不符

**问题描述**
transport 文档写 Windows 使用 overlapped I/O 或 `CancelIoEx` 完成阻塞与取消。实际同步命名管道上挂起的 `ReadFile` 会阻塞同一连接的写，必须改为 `PeekNamedPipe` 轮询读；accept 的阻塞取消则需要 `CancelSynchronousIo`，单独 `CancelIoEx` 不能唤醒同步 `ConnectNamedPipe`。

**解决方法**
- Windows 读路径改为 `PeekNamedPipe` 查询可用字节，再同步 `ReadFile`；共享取消标志由 `shutdown` 设置。
- accept 线程复制自己的线程句柄，`shutdown` 时调用 `CancelSynchronousIo`。
- 增加“双方同时在读时仍可双向写”的回归测试。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-transport/PROVIDER.html` | §2 CONVENTIONS（阻塞与取消） | 改 | 改为“accept 用 CancelSynchronousIo；read 用 PeekNamedPipe 轮询 + 取消标志”。 |
| `crates/voime-transport/CORE.html` | §2 CONVENTIONS | 增 | 增加 Windows 读写约束：挂起读会阻塞同连接写，因此读必须先探测可用字节。 |
| `crates/voime-transport/CORE.html` | §5.2 accept 与关闭 | 改 | 调用栈补上 `CancelSynchronousIo`。 |
| `PLATFORM.html` | §5.2 平台传输实现 | 不动 | 只描述平台差异与安全边界，具体取消调用由 transport 文档拥有。 |

### 1.5 Windows 安装产物写成 `voime-setup.exe`

**问题描述**
Windows BUILD 原文把安装流程写成目标 `voime-setup.exe`，但本仓库没有 Inno Setup / MSI 构建链。实际交付物是 PowerShell 安装器、卸载器与模型安装脚本；继续按旧文档找安装器会找不到产物。

**解决方法**
- 提供 `packaging/windows/install.ps1`、`uninstall.ps1` 与 `scripts/install.ps1`、`scripts/install_models.ps1`。
- 安装脚本完成构建、复制二进制、登录启动、模型安装；卸载脚本清理安装目录、启动项，并支持按需删除设置与模型。
- 文档改为 PowerShell 命令，并写明 `-NoBuild`、`-NoModels`、`-NoStartup`、`-Prefix` 等实际参数。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/BUILD.html` | §3 安装与启动 | 改 | 用 PowerShell 安装/卸载命令替换 `voime-setup.exe`，列出参数与默认目录。 |
| `BUILD.html` | §3.6 Windows 从源码安装；§8 升级 / 卸载 | 改 | 增加脚本路径、默认前缀、登录启动与卸载选项。 |
| `README.html` | §2 用户交互（安装 / 启动） | 不动 | 只描述用户可见结果，具体脚本入口由 BUILD 拥有。 |

## 2. 开发中发现文档没写明的细节

### 2.1 缺少 Fcitx 等价的 Windows 设置窗口契约

**问题描述**
Windows USER 只规定“托盘菜单设置 Mode / Hotkey”，没有定义独立配置窗口。实现时无法确定哪些选项必须出现、如何保存、如何取消，也无法保证和 Fcitx 配置项一一对应。

**解决方法**
- 新增 `settings.rs`：模式下拉框、热键输入框、浮层与登录启动复选框、daemon 启动 / 停止 / 诊断按钮、保存 / 取消。
- 设置窗口在独立线程运行，保存通过通道交回主线程应用，避免与热键注册、浮层和 IPC 状态竞争。
- UI 章节补充窗口尺寸、字体、配色与控件契约。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §4.1 配置；§5 UI | 改 | 增加设置窗口入口、字段、保存语义与视觉契约。 |
| `frontends/windows/CORE.html` | §1 文件树；§4.10 settings.rs；§5.6 | 增 | 增加设置窗口类型、参数与线程/保存调用栈。 |
| `frontends/windows/README.html` | §2 用户交互（配置） | 改 | 写明“托盘右键菜单或设置窗口”。 |
| `README.html` | §2 用户交互（配置） | 改 | Windows 配置入口补上设置窗口。 |

### 2.2 daemon 手动启停与登录启动细节不足

**问题描述**
文档只说“托盘拉起 daemon”，没有写明用户如何手动启动、停止、诊断 daemon，也没有说明安装器使用哪种登录启动机制、停止操作走什么命令。实现时容易把 daemon 做成 Windows 服务或让托盘退出时误杀 daemon。

**解决方法**
- 明确每用户 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 启动 `voime-tray.exe`，不使用 Session 0 服务。
- 托盘自动启动同目录 `voimed.exe`；右键菜单与设置窗口提供“启动 daemon / 停止 daemon / 诊断”。
- 停止操作通过 `voime ctl quit`；退出托盘只关闭前端与浮层，daemon 继续运行，需单独停止。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §4.6 daemon 生命周期 | 增 | 增加登录启动、自动拉起、手动启停、诊断、退出托盘的行为表。 |
| `README.html` | §2 用户交互（停止 daemon） | 改 | 增加 Windows 托盘菜单 / 设置窗口的停止入口。 |
| `frontends/windows/BUILD.html` | §3 安装与启动 | 改 | 写明登录启动注册表项、托盘拉起 daemon、停止命令。 |
| `crates/voimed/BUILD.html` | §1 环境（服务管理） | 改 | 把 Windows 目标改为“登录启动 / 托盘拉起已实现”。 |

### 2.3 配置路径与保存方式未固定

**问题描述**
Windows USER 说“设置载体由实现决定”，而 WINDOWS 附录给出 `%APPDATA%\voime\config.json`。如果不固定路径，doctor、安装器和用户排障无法给出确定位置；保存实现也容易在 Windows 文件替换语义上出错。

**解决方法**
- 固定配置路径为 `%APPDATA%\voime\config.json`。
- 使用临时文件写入后替换；替换前删除已有文件，避免 Windows 上 `rename` 不能覆盖目标。
- 设置窗口保存时先校验热键字符串，再由主线程统一应用并发送 `Configure`。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §4.1 配置 | 改 | 写明配置路径、临时文件写入与立即同步语义。 |
| `frontends/windows/CORE.html` | §4.2 config.rs | 改 | 写明 `save` 的原子替换与 Windows 覆盖行为。 |
| `WINDOWS.html` | §9 附录映射 | 不动 | 路径已在此给出；前端 USER 只引用，不重复定义。 |

### 2.4 浮层缺少可实现的数值契约

**问题描述**
Windows USER §5 明确要求“实现前补充几何、字体、圆角与多显示器定位数值”，但没有给出任何基准值，导致浮层实现和验收无法逐项对照。

**解决方法**
- 以 96 DPI 为基准实现内边距 10、最小宽度 120、最大文本宽度 560、最小高度 20，按目标窗口 DPI 缩放。
- 字体固定为 `DEFAULT_GUI_FONT`，颜色使用 `COLOR_INFOTEXT` / `COLOR_INFOBK`，整体不透明度 235/255，方角。
- 定位优先跟随目标光标下方 4 px，无光标时放在窗口左下外侧，并限制在工作区内。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §5 UI | 改 | 写入上述几何、字体、颜色、不透明度与定位契约。 |
| `WINDOWS.html` | §4.1 / §4.4 | 不动 | 只保留方案对比与风险；浮层数值契约由 windows USER §5 唯一拥有。 |
| `frontends/windows/CORE.html` | §4.6 preview.rs | 不动 | 签名不变；数值属于 USER 的视觉契约。 |

### 2.5 启动顺序与热键注册的窗口依赖未说明

**问题描述**
文档的启动调用栈把 IPC 连接放在 `Tray::create` 和热键注册之前，但 `RegisterHotKey` 需要一个窗口句柄。实现必须先在主线程创建托盘窗口，再注册热键，否则热键无法注册。

**解决方法**
- 启动顺序改为：单实例 → 加载配置 → 创建托盘窗口 → 创建浮层 → 连接 daemon → 注册热键 → 进入消息循环。
- 托盘窗口同时承担通知区回调、`WM_HOTKEY` 和菜单消息循环。
- 文档 5.1 调用栈更新为实际顺序。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/CORE.html` | §5.1 启动与连接 | 改 | 按实际顺序列出 Tray → Overlay → try_connect → sync_hotkey。 |
| `frontends/windows/CORE.html` | §4.1 main.rs | 改 | 写明 `ensure_single_instance` 后可先建窗口/托盘，再注册热键。 |
| `frontends/windows/USER.html` | §2 CONVENTIONS | 不动 | 用户契约只关心“热键可用”，不关心窗口创建顺序。 |

### 2.6 Windows 模型安装选项与缓存目录未说明

**问题描述**
文档只给出模型目标目录，没有说明 Windows 缓存目录、独立模型安装脚本、跳过构建/模型的安装参数。用户排障时容易把缓存、虚拟环境和模型目录混在一起。

**解决方法**
- `tools/install_models.py` 在 Windows 默认使用 `%LOCALAPPDATA%\voime\cache` 与 `%LOCALAPPDATA%\voime\models`。
- `scripts/install_models.ps1` 在缓存目录创建 venv，按 requirements 哈希复用依赖。
- 主安装器支持 `-NoModels`、`-NoBuild`、`-NoStartup`、`-Prefix`，卸载器支持 `-RemoveSettings`、`-RemoveModels`。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `BUILD.html` | §4 安装模型 | 改 | 增加 Windows 命令、缓存目录、venv 与模型目录的关系。 |
| `frontends/windows/BUILD.html` | §3 安装与启动 | 增 | 增加模型安装与跳过模型的参数说明。 |
| `README.html` | §2 用户交互 | 不动 | 只描述安装结果，不列脚本参数。 |

## 3. 其它技术上踩的坑

### 3.1 同步命名管道的挂起读会阻塞同连接写

**问题描述**
托盘启动后显示“未连接”，右键盘菜单也没有响应；daemon 日志显示已收到 `Hello` 并发送 `Snapshot`，但客户端一直收不到，`voime ctl status` 也会挂住。根因是 Windows 同步命名管道上，一个已挂起的 `ReadFile` 会阻塞同一连接上的写；服务端 reader 线程在等客户端输入时，writer 线程无法把 `Snapshot` 写出去，托盘主线程卡在握手阶段，消息循环随之停摆。

**解决方法**
- 读路径改为 `PeekNamedPipe` 轮询可用字节，只有有数据时才调用 `ReadFile`，避免长期挂起的同步读。
- `Shared` 增加取消标志，`shutdown` 设置标志并调用 `CancelIoEx`，轮询循环可退出。
- 增加“双方同时挂起读，仍能双向写”的 transport 回归测试。
- 修复后实测托盘 `addon=Connected`，左键可打开设置窗口，退出可正常回收。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-transport/PROVIDER.html` | §2 阻塞与取消 | 改 | 写明同步命名管道挂起读会阻塞写，因此 Windows 读采用 PeekNamedPipe 轮询。 |
| `crates/voime-transport/CORE.html` | §2 CONVENTIONS | 增 | 增加 Windows 读写约束和取消标志说明。 |
| `crates/voime-transport/USER.html` | §4.2 Listener / Stream | 不动 | 对外契约仍是“read 可阻塞、shutdown 可唤醒”，不暴露轮询细节。 |

### 3.2 `CancelIoEx` 不能唤醒同步 `ConnectNamedPipe`

**问题描述**
listener 关闭时只调用 `CancelIoEx`，但 accept 线程阻塞在同步 `ConnectNamedPipe`，`shutdown` 后线程仍不退出，daemon 关闭会卡住。

**解决方法**
- accept 线程复制当前线程句柄，记录到 listener 状态。
- `shutdown` 对阻塞线程调用 `CancelSynchronousIo`，同时保留 `CancelIoEx` 清理连接 I/O。
- 增加“阻塞 accept 能被 shutdown 唤醒”的回归测试。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-transport/PROVIDER.html` | §2 阻塞与取消 | 改 | 明确同步 `ConnectNamedPipe` 的取消使用 `CancelSynchronousIo`。 |
| `crates/voime-transport/CORE.html` | §5.2 accept 与关闭 | 改 | 调用栈增加 `CancelSynchronousIo`。 |
| `PLATFORM.html` | §5.2 平台传输实现 | 不动 | 只列平台差异，不复制具体 Win32 调用。 |

### 3.3 windows-sys 的 `DuplicateHandle` 与 `HKEY` 类型

**问题描述**
按直觉把 `DuplicateHandle` 放在 `Win32::System::Threading` 会编译失败；它实际在 `Win32::Foundation`。另外 `HKEY` 在当前 `windows-sys` 中是裸指针，不能用 `0` 初始化，需要 `std::ptr::null_mut()`。

**解决方法**
- 修正 `DuplicateHandle` 导入模块。
- `RegOpenKeyExW` 的 `HKEY` 初始化改为 `null_mut()`。
- 通过 MSVC 编译与 clippy 验证导入路径。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-transport/CORE.html` | §4.3 windows.rs | 不动 | 签名不变；导入路径属于实现细节，不进入契约。 |
| `crates/voime/CORE.html` | §4.7 doctor.rs | 不动 | doctor 的公开行为不变；不记录底层 Win32 类型。 |

### 3.4 `windows` crate 控件样式与空句柄

**问题描述**
`CBS_DROPDOWNLIST`、`BS_AUTOCHECKBOX`、`ES_AUTOHSCROLL` 是 `i32` 常量，而 `WS_TABSTOP`、`WS_BORDER` 是 `WINDOW_STYLE` 新类型，不能直接位或。`CreateWindowExW` 的父窗口、菜单、实例参数也不能随意传 `None`，否则泛型无法推导。

**解决方法**
- 组合样式时把 `WINDOW_STYLE.0` 转成 `i32` 再合并。
- 顶层窗口使用 `HWND::default()`、`HMENU::default()`、`HINSTANCE::default()` 表示空句柄。
- 子控件菜单句柄用 `HMENU(id as usize as *mut c_void)` 传递控件 ID。
- 用 `WM_SETFONT` + `DEFAULT_GUI_FONT` 统一系统字体。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/CORE.html` | §4.10 settings.rs | 不动 | 窗口/控件创建与类型转换属于实现细节，不改变设置窗口契约。 |
| `frontends/windows/USER.html` | §5 UI | 不动 | 视觉契约只描述尺寸、字体、控件关系，不描述 Win32 类型。 |

### 3.5 Windows 上 `fs::rename` 不能覆盖已有文件

**问题描述**
配置第一次保存成功，第二次保存时 `fs::rename(temp, config.json)` 在 Windows 上失败，因为目标已存在。用户修改 Mode / Hotkey 会表现为“保存失败”。

**解决方法**
- 先写 `.json.tmp`，若目标存在则先 `remove_file`，再 `rename`。
- 保留错误上下文；失败时设置窗口显示 MessageBox。
- 模型安装器继续使用已有的备份/替换流程。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/CORE.html` | §4.2 config.rs | 改 | 写明临时文件 + 删除旧文件 + rename 的保存流程。 |
| `frontends/windows/USER.html` | §4.1 配置 | 不动 | 只规定持久化结果与立即同步，不规定文件系统实现。 |

### 3.6 `SetWindowTextW` 需要 NUL 结尾

**问题描述**
浮层把 `text.encode_utf16().collect()` 直接传给 `SetWindowTextW`，字符串没有结尾 NUL，Windows 会继续读到缓冲区之外，可能显示乱码或崩溃。

**解决方法**
- 测量文本时使用无 NUL 的 UTF-16 切片。
- 设置窗口文本前重新追加 `0u16`，再传 `PCWSTR`。
- 所有 registry、菜单、消息框字符串统一走带 NUL 的 `to_wide`。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/CORE.html` | §4.6 preview.rs | 不动 | 签名与行为契约不变；NUL 终止是 Win32 实现细节。 |
| `frontends/windows/USER.html` | §5 浮层内容 | 不动 | 用户只看到纯文本，不涉及字符串内存布局。 |

### 3.7 `RegisterHotKey` 没有 key-up，且会受自动重复影响

**问题描述**
PTT 需要松手停止，但 `RegisterHotKey` 只提供按下通知；Toggle 按住键时还可能收到重复通知，造成连续切换。若直接把 `WM_HOTKEY` 当点击，会出现录音停不下来或 Toggle 反复开关。

**解决方法**
- 用 `GetAsyncKeyState` 在主循环轮询 key-up，PTT 释放后发送 `Stop`。
- 用 `hotkey_down` 去重，忽略按住期间的重复 `WM_HOTKEY`；key-up 后复位。
- 进入 finalizing 后忽略新的热键启动，避免和 daemon 的 `Busy` 竞争。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §2 CONVENTIONS；§3.1 后台听写 | 改 | 明确 PTT key-up 和 Toggle 去重语义。 |
| `frontends/windows/PROVIDER.html` | §4.1 Win32 / UIA 调用 | 改 | 写明 `GetAsyncKeyState` 是当前 key-up 轮询方案。 |
| `frontends/windows/CORE.html` | §5.2 热键到提交 | 改 | 调用栈补上 key-up 轮询与重复按键过滤。 |

### 3.8 托盘隐藏窗口无法可靠成为前台

**问题描述**
右键菜单需要 owner 窗口成为前台才能正常显示和消失；最初托盘窗口完全隐藏，`SetForegroundWindow` 可能失败，菜单会不显示或点击外部不消失。

**解决方法**
- 创建 `WS_EX_TOOLWINDOW` 的 1×1 顶层窗口，放在屏幕外并 `SW_SHOWNOACTIVATE`，保留可成为前台的能力。
- 菜单显示前 `SetForegroundWindow`，菜单结束补发 `WM_NULL`。
- 左键托盘直接打开设置窗口，右键打开菜单；退出项和 `WM_CLOSE` 都进入正常关闭路径。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §5 UI（托盘图标） | 改 | 写明左键设置、右键菜单、退出行为与菜单项。 |
| `frontends/windows/CORE.html` | §4.8 tray.rs | 不动 | 窗口实现技巧不改变 Tray 的公开签名，设置窗口契约由 §4.10 拥有。 |
| `frontends/windows/README.html` | §2 用户交互 | 不动 | 只描述用户入口，不复制托盘窗口实现。 |

### 3.9 UIA RuntimeId 与 COM 线程模型

**问题描述**
Windows 没有 Fcitx `InputContext` 等价物，目标绑定依赖 UIA 焦点元素。UIA 返回 `SAFEARRAY`，需要在 COM 初始化后读取；如果直接按 Rust `Vec` 处理或忽略 COM 线程模型，会拿不到 RuntimeId 或崩溃。

**解决方法**
- 主线程以 STA 调用 `CoInitializeEx`，用 `CUIAutomation` 获取焦点元素。
- 读取 `SAFEARRAY` 的 LBound/UBound、访问数据后 `SafeArrayUnaccessData`，最后 `SafeArrayDestroy`。
- UIA 不可用时降级为只绑定前台 HWND；提交前仍复核 HWND 和 RuntimeId。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/PROVIDER.html` | §4.1 Win32 / UIA 调用 | 改 | 补充 COM 初始化、RuntimeId 的 SAFEARRAY 生命周期与 UIA 不可用时的降级。 |
| `frontends/windows/USER.html` | §4.2 目标与提交 | 不动 | 仍规定“HWND + RuntimeId，UIA 不可用时尽力复核”。 |
| `frontends/windows/CORE.html` | §4.5 target.rs | 改 | 补上 `current_runtime_id` 的 SAFEARRAY 读取与销毁说明。 |

### 3.10 SendInput 的 UTF-16、代理对与部分失败

**问题描述**
文本内部是 UTF-8，`SendInput` 需要 UTF-16 单元；emoji 是代理对，按字节或随机边界切分会产生半个字符。长文本一次性发送还可能遇到目标应用处理异常或 UIPI 导致部分事件失败。

**解决方法**
- 统一 `encode_utf16()` 后按 UTF-16 单元注入，每单元发送按下/抬起事件。
- 分块时若块尾是高位代理，则把完整代理对留在同一块。
- 检查 `SendInput` 返回数量；不足时停止后续注入并记录错误，不重试其他窗口。
- 提交前再次调用 `target::focused`，目标变化则放弃。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §4.2 目标与提交 | 改 | 明确代理对不拆分、分批注入、部分失败不重试。 |
| `frontends/windows/CORE.html` | §4.7 commit.rs | 改 | 增加 `to_utf16`、分块与 `send_unicode` 的错误处理签名说明。 |
| `WINDOWS.html` | §4.1 实现要点 | 不动 | 只保留 SendInput 的平台风险与覆盖边界，详细契约在 windows USER/CORE。 |

### 3.11 `ort` 在 Windows GNU 目标没有预编译包

**问题描述**
用 `x86_64-pc-windows-gnu` 做快速检查时，`ort-sys` 直接报“does not provide prebuilt binaries for the target”。这不是代码错误，而是 Windows 推理依赖要求 MSVC 目标；继续在 GNU 下排查会浪费时间。

**解决方法**
- 使用 `x86_64-pc-windows-msvc` + Visual Studio Build Tools 验证 daemon/CLI。
- 前端与 transport 可先用 GNU 做快速反馈，但最终验收以 MSVC 为准。
- 文档明确 Windows 构建使用 MSVC 工具链。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `BUILD.html` | §1 环境（Windows 构建依赖） | 改 | 写明 Windows 只支持 MSVC；GNU 目标没有 ORT 预编译包。 |
| `frontends/windows/BUILD.html` | §1 环境 | 不动 | 前端不依赖 ONNX，GNU/MSVC 均可；只在根文档约束 daemon。 |

### 3.12 XDG/HOME 路径在 Windows 失效

**问题描述**
`voime-core::config::default_model_dir` 和 Python 模型安装器最初只读 `XDG_DATA_HOME` / `HOME`，Windows 上会退到相对目录 `models`，导致 daemon 找不到模型、doctor 报错。

**解决方法**
- Rust 在 Windows 优先使用 `LOCALAPPDATA\voime\models`。
- Python 安装器的默认模型目录与缓存目录在 Windows 使用 `LOCALAPPDATA\voime\...`。
- doctor 输出实际模型目录，安装脚本与 daemon 使用同一默认值。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `BUILD.html` | §4 安装模型（默认目录） | 不动 | 已明确 Windows 使用 `%LOCALAPPDATA%\voime\models`。 |
| `crates/voime-core/USER.html` | §4.1 配置 | 改 | 增加 Windows 默认路径与 XDG 路径的优先级。 |
| `frontends/windows/CORE.html` | §4.2 config.rs | 不动 | 前端配置路径与模型路径分离，模型路径由 core/USER 拥有。 |

## 4. 后续 ISSUES 修复的 BUG

### 4.1 TSF（P4）仍未实现

**问题描述**
当前交付是后台听写 MVP；需要“被系统选为输入法”的场景还没有 TSF DLL、COM 注册、多客户端路由和进程内编辑会话。

**解决方法**
- 保持 `frontends/windows/tsf` 为可选子工程，不进入 Rust workspace。
- 先完成单应用 TSF 原型，再决定 keyboard TIP / speech TIP；不假设后台旁路。
- 验证 x86/x64 DLL、多应用加载、升级时旧 DLL 残留和编辑会话失败。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/README.html` | §1.3 非目标；状态 | 不动 | 已标明 P4 可选且当前分支未包含。 |
| `frontends/windows/CORE.html` | §4.9 tsf/ | 不动 | 保留目标接口草案，等 P4 实现时再落代码。 |
| `WINDOWS.html` | §6.2 P1（决定做 TSF 后） | 不动 | 作为后续决策清单保留，不与 P3 验收混淆。 |

### 4.2 daemon 常开麦克风与蓝牙耳机策略

**问题描述**
daemon 启动即打开输入流并常驻，Windows 会常亮麦克风指示；蓝牙耳机可能被拉入 HFP，隐私设置拒绝或设备拔出时用户只看到“未连接”。

**解决方法**
- 后续评估按段开流/关流，至少增加可配置的音频生命周期。
- doctor 和托盘应展示实际设备、隐私拒绝错误与设备切换错误。
- 蓝牙设备策略单独做验收，不把“能启动”当作“可用”。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `WINDOWS.html` | §6.1 P0 #4 音频适配；§7.1 风险 | 不动 | 已记录按段开流与蓝牙风险，作为后续 issue 来源。 |
| `crates/voime-core/USER.html` | §4.2 音频采集 | 增 | 后续补上音频生命周期与设备选择契约。 |
| `frontends/windows/USER.html` | §4.6 daemon 生命周期 | 增 | 增加“失败原因展示”与音频设备错误的用户提示。 |

### 4.3 daemon 启动失败不可见，重试可能反复拉起

**问题描述**
托盘只显示“daemon 未连接”；如果 `voimed.exe` 因模型缺失、音频占用或隐私设置退出，用户没有直接错误。当前重连逻辑还会按间隔反复 spawn daemon，可能造成重复启动和模型加载压力。

**解决方法**
- 后续记录 daemon stderr 到 `%LOCALAPPDATA%\voime\logs`，或把首行错误回传给托盘状态。
- 用 PID/退出码跟踪启动结果，避免失败后无界重试；增加退避。
- 设置窗口显示最近错误，并提供“诊断”和“打开日志”。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §4.4 诊断；§4.6 daemon 生命周期 | 增 | 增加启动失败原因、日志位置与重试策略。 |
| `frontends/windows/CORE.html` | §5.6 设置窗口与 daemon 启停 | 改 | 增加 `start_daemon` 的退出码/日志反馈。 |

### 4.4 SendInput 在真实应用中的覆盖矩阵未完成

**问题描述**
当前只验证了编译、IPC 和目标复核逻辑，没有在中文输入法开启、Chromium/Electron、Office、Windows Terminal、UWP、提权窗口、游戏等真实目标中逐项验证。

**解决方法**
- 按 WINDOWS §7.2 的矩阵执行手工验收，记录每个应用的通过/降级/不承诺状态。
- 中文输入法开启时重点验证 Unicode 注入是否被吞；提权窗口记录为不注入。
- 失败场景保留 daemon 历史；必要时再做 TSF 或白名单回退。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `WINDOWS.html` | §7.2 测试矩阵；§7.4 未知项 | 不动 | 保留完整矩阵，作为后续验收 issue。 |
| `frontends/windows/USER.html` | §4.5 覆盖与错误 | 不动 | 已声明各应用边界；等实测数据再更新“需实测”项。 |
| `BUILD.html` | §6 测试与验收 | 改 | 增加“Windows 应用矩阵”作为发布门槛。 |

### 4.5 设置窗口的热键冲突反馈滞后

**问题描述**
设置窗口保存时只校验格式；真正的 `RegisterHotKey` 冲突在主线程应用设置后才发现，此时设置窗口已经关闭，用户需要重新打开才能修改。

**解决方法**
- 后续让设置窗口保存时同步等待主线程的“应用结果”，失败则保留窗口并显示错误。
- 或提前在设置线程用临时窗口尝试注册热键，再正式应用；注意排除当前窗口已占用的组合。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §4.1 配置 | 增 | 写明热键注册冲突时的用户反馈与窗口保留策略。 |
| `frontends/windows/CORE.html` | §5.6 设置窗口与 daemon 启停 | 增 | 增加 Save 请求/响应通道与失败回滚调用栈。 |

### 4.6 DPI 感知与多显示器浮层定位仍需验证

**问题描述**
浮层按目标窗口 DPI 计算尺寸，但进程没有显式声明 per-monitor DPI awareness，且多显示器、混合缩放、光标矩形边界还没有完整实测。

**解决方法**
- 启动时设置 `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)`，或使用应用 manifest。
- 在不同 DPI、多显示器、最大化/最小化目标窗口下验证浮层尺寸和位置。
- 用 `GetGUIThreadInfo` 光标矩形失败时回退到窗口矩形。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §5 UI（几何 / 定位） | 增 | 增加 DPI awareness 与多显示器验收要求。 |
| `frontends/windows/PROVIDER.html` | §4.1 DPI / monitor API | 改 | 写明进程 DPI 模式与目标窗口 DPI 的关系。 |

### 4.7 命名管道轮询读的技术债

**问题描述**
当前 Windows 读使用 5 ms 间隔的 `PeekNamedPipe` 轮询，解决了同步读阻塞写的问题，但每个客户端空闲时仍有周期唤醒；大量客户端或长时间运行时会产生不必要的 CPU 唤醒。

**解决方法**
- 后续可改为 overlapped I/O + 事件等待，保留 shutdown 取消；
- 或按连接改为阻塞读 + 独立写句柄/线程模型，先做并发压测再替换。
- 保留双平台 transport conformance 测试，防止回退到会死锁的同步读。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-transport/PROVIDER.html` | §2 阻塞与取消 | 不动 | 当前实现已记录；等 overlapped 替换时再更新。 |
| `crates/voime-transport/CORE.html` | §2 Windows 读写 | 不动 | 轮询是实现细节，不改变 Stream 契约。 |

### 4.8 停止 daemon 依赖外部 `voime.exe`

**问题描述**
托盘“停止 daemon”通过启动同目录 `voime.exe ctl quit` 实现；如果安装不完整、`voime.exe` 缺失或版本不匹配，停止按钮无效，且托盘不显示子进程失败原因。

**解决方法**
- 后续直接在托盘内建立 `Role::Control` 连接并发送 `Quit`，复用 `voime-transport`，不依赖外部进程。
- 保留 `voime ctl quit` 作为 CLI 入口和故障兜底。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/CORE.html` | §4.3 ipc.rs；§5.6 daemon 启停 | 增 | 增加 Control 连接发送 Quit 的客户端路径。 |
| `frontends/windows/USER.html` | §4.6 daemon 生命周期 | 改 | 停止 daemon 的契约从“调用 voime ctl quit”改为“前端直接发送 Quit”。 |

### 4.9 模型安装依赖 Python、网络与较大磁盘

**问题描述**
Windows 主安装器默认安装模型时需要 Python 3.10+、网络和约 740 MiB 以上空间；当前没有断点续传、镜像回退或离线校验包，首次安装失败后用户体验较差。

**解决方法**
- 后续把模型安装拆成显式可选步骤，提供断点/重试与本地包校验。
- 主安装器默认只装二进制，模型由用户选择或由发布包附带校验脚本。
- 在设置/诊断中显示模型目录、缺失文件与 SHA 校验结果。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `BUILD.html` | §4 安装模型 | 改 | 写明 Windows 的 Python/网络/磁盘前置条件与失败恢复方式。 |
| `frontends/windows/BUILD.html` | §3 安装与启动 | 改 | 安装器默认行为与 `-NoModels` 的取舍写清楚。 |

### 4.10 Linux 回归尚未在本环境执行

**问题描述**
本次只在 Windows 上运行了完整测试、clippy、fmt 与 release 构建；虽然共享 crate 的代码路径保持 cfg 隔离，但没有在本机 Linux/Fcitx5 环境重新跑 Linux 验收。

**解决方法**
- 在 Linux CI 或开发机执行 `cargo test --workspace`、Fcitx5 addon CMake/ctest 与手工验收。
- 重点检查 `Endpoint::from_path`、`doctor` 的 Linux 分支、systemd 安装脚本和 Fcitx 提交路径。
- 双平台共享测试保持同一组协议/分段/状态用例。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `BUILD.html` | §6 测试与验收 | 改 | 增加 Linux 回归作为 Windows 改动的合并门槛。 |
| `PLATFORM.html` | §6.5 CI 矩阵；§8 验收规则 | 不动 | 已定义双平台 CI 与“Linux 不回归”规则。 |
