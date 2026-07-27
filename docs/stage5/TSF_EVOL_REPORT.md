# Voime 阶段五：TSF 架构演进报告（P5-01~07，M5）

日期：2026-07-27　分支：`voime-ng-stage5`

## 1. 交付总览

阶段五完成控制反转架构演进：Agent 从"主动触发者"变为"被动计算服务"，
热键监听与文本注入职责移交标准 Windows 文本服务（TSF DLL）。

| 产物 | 说明 |
| --- | --- |
| `voime_input.exe` | 标准版 Agent：IPC 服务端 + 音频 + 悬浮窗；**无 SendInput/热键代码**（objdump 验证无导入） |
| `voime_tsf.dll` | TSF 文本服务：ITfTextInputProcessor + ITfKeyEventSink + IPC 客户端 + TSF 注入 |
| `voime_input_notsf.exe` | NOTSF 调试版：本地热键 + SendInput，无 IPC/TSF（P5-06 配置分离） |
| `voime_input_console.exe` | 控制台自测版（NOTSF；自测 15/15 PASS） |
| `frontend/tsf/register_tsf.ps1` / `unregister_tsf.ps1` | 自提权注册/注销脚本 |
| 绿色包 | `frontend/dist/voime_input-1.1.0-tsf-win64.zip` |

## 2. 任务验收状态

| 任务 | 验收标准 | 状态 | 证据 |
| --- | --- | --- | --- |
| P5-01 TSF DLL 框架 | 语言栏出现 "Voime Input"，选中不报错 | ✅（自动化） | qa06.regsvr32/reg_com/reg_tip；注册后经语言栏切换在 harness/记事本激活（tsf 日志 `Activate tid=18`） |
| P5-02 IPC Client（TSF 侧） | 加载即连、断线自动重连、ACL 拒绝非当前用户 | ✅ | qa06.tsf_connect；重连逻辑经 qa05 断连场景覆盖（服务侧观察）；ACL 仅当前用户 SID+SYSTEM（跨用户拒连未自动化，需第二账户手工验证） |
| P5-03 IPC Server（Agent 侧） | 监听、多客户端连接池 | ✅ | qa05 全 PASS（连接/应答/多客户端/重连）；日志 `Client Connected (id=N pid=M)` |
| P5-04 控制反转按键拦截 | PTT 按下/松开 -> IPC start/stop -> Agent 状态机 | ✅ | qa06.event_seq；F13 PTT 经 ctfmon->ITfKeyEventSink->IPC 全链路 |
| P5-05 文本注入 | commit_text -> 光标处出现文本并返回成功 | ✅ | qa06.inject_text 文本精确匹配、注入响应 hr=0x0；松开->上屏 312ms（目标<500ms） |
| P5-06 工程配置分离 | 标准版无 SendInput 导入；NOTSF 版热键可输入、无 TSF 产物 | ✅ | objdump 导入表验证；NOTSF 版 qa01/qa02 回归 PASS |
| P5-07 管理员权限适配 | 提权应用（CMD/任务管理器）可加载输入 | ⚠️ 自动化部分通过 | 见 §5 |

## 3. 架构与关键设计

### 3.1 进程/组件视图（与 PRD 2.1.2 一致）
- `VoimeAgent.exe`：IPC Server（`frontend/src/ipc_server.*`）-> 统一触发入口
  （`App::trigger_start/stop`）-> Rust 引擎 -> `commit_text` 回投给发起会话的客户端。
- `VoimeTSF.dll`（`frontend/tsf/src/`）：`dllmain`（COM 入口）+ `register.cpp`
  （CLSID/Profile/Category 注册）+ `text_service.*`（TIP 本体）+ `ipc_client.*`
  （管道客户端）+ `ptt.*`（读取 Agent 同一份 config.toml 的 hotkey）。
- IPC 协议（`frontend/src/ipc_proto.h`，PRD 2.3.3）：消息模式 Named Pipe
  `\\.\pipe\VoimeInput.IPC`，一条 JSON 一消息；`start_session/stop_session`
  （TSF->Agent）、`commit_text`（Agent->TSF）、`{request_id,status,code}` 应答；
  request_id 为 BCrypt 随机 uuid。
- 管道 ACL：SDDL `D:P(D;;GA;;;NU)(D;;GA;;;AN)(A;;GA;;;<当前用户SID>)(A;;GA;;;SY)`，
  仅当前用户（含其提权进程）可连，拒绝网络/匿名。

### 3.2 文本注入路径（P5-05）
IPC reader 线程收到 `commit_text` -> PostMessage 到 TIP 隐藏窗口（UI 线程/套间）
-> `RequestEditSession(TF_ES_ASYNC|TF_ES_READWRITE)` -> `DoEditSession` 内
`GetSelection` + `range->SetText` 直接落字 -> `DoEditSession` 内 IPC 应答 Agent。

## 4. 调试实录（关键坑与结论，后续维护必读）

1. **同步管道句柄读写死锁**：同步句柄同一时刻只允许一个 I/O —— 读线程阻塞
   ReadFile 时，另一线程 WriteFile 永久阻塞。两侧（Agent Server/TSF Client）
   均须 `FILE_FLAG_OVERLAPPED` + 真 OVERLAPPED 结构读写（qa05 实测复现）。
2. **重叠句柄配 NULL lpOverlapped 是未定义行为**：表现为偶发零字节"空消息"
   与消息丢失。所有重叠句柄读写必须携带 OVERLAPPED+事件。
3. **python 管道客户端两坑**：`PIPE_READMODE_MESSAGE=0x2`（bit0=NOWAIT，
   误置后 ReadFile 立即 232 ERROR_NO_DATA）；Peek 轮询单线程收发替代读线程。
4. **TSF Profile 注册必须提权**：`RegisterProfile` 写 `HKLM\SOFTWARE\Microsoft\
   CTF\TIP`，非提权返回 E_FAIL（实测 Win10 22H2）。PRD 5.1 纯 HKCU 注册对
   Profile 不可行；COM 类（HKCU\Software\Classes\CLSID）免提权且提权进程可见。
   另：`RegisterProfile` 不覆盖已有启用状态，需 `EnableLanguageProfile` 自愈。
5. **编辑会话三连坑**：① `TF_ES_SYNC` 在本线程拥有目标文档时自死锁（永久挂起）；
   ② 异步会话对象所有权——请求方提前 Release 导致永不派发，须"成功移交+
   DoEditSession 末尾自释"；③ `DoEditSession` 内禁止 `GetFocus/GetTop`
   （TSF 串行锁二次进入死锁），目标上下文须请求前取好。
6. **`ITfInsertAtSelection::InsertTextAtSelection` 在本机标准 EDIT 上永久阻塞**
   （原因未明）；改 IME 惯例 `GetSelection`->`SetText`（读写会话内直接改文档），
   回读验证文本真实落文档。`StartComposition` 传 NULL sink 在本机返回
   E_INVALIDARG（须带 sink，即使空实现）。
7. **Profile 激活**：`ProfileMgr::ActivateProfile` 行为不稳（zh 偶发 E_INVALIDARG、
   en 空成功不实例化）；ctfmon profile 缓存需数秒刷新，注册后立刻激活必败；
   反复注册/注销会搞坏缓存。**可靠路径 = Win+Space 语言栏切换**（qa06 采用），
   v1 `ChangeCurrentLanguage+ActivateLanguageProfile` 次之。
8. **前台锁与焦点**：跨进程 `SetForegroundWindow/SetFocus` 不可靠（本线程 SetFocus
   对异线程窗口无效）；真实输入事件（自点击窗口）是前台锁唯一认可的途径；
   `AllowSetForegroundWindow(pid)` + 目标进程自持焦点为辅助手段。
9. **PTT 组合避让**：`Ctrl+Alt+T` 在本机被某软件 RegisterHotKey 全局占用 ——
   LL 钩子（NOTSF）能见到按键，但 **TSF 按键槽永远收不到**（被抢占于 ctfmon
   之前）。产品侧注意：PTT 配置应避开全局热键（qa06 用 F13 验证）。
10. **TIP 文件锁**：TIP 一旦被 explorer 等常驻进程加载即锁文件，开发迭代时
    注册用沙盒副本（唯一文件名），仓库 build 目录不受影响。

## 5. P5-07 管理员权限适配（验收状态详述）

自动化已通过（qa07 全 PASS）：
- HKLM Profile + `GUID_TFCAT_TIPCAP_SECUREMODE` 类别注册 —— 提权进程可见可加载；
- **提权客户端经管道 ACL（同用户跨 IL）完成全 IPC 会话并收到正确识别文本**——
  即 TSF 加载进提权进程后所走的真实通道；
- TIP 可加载进任意进程（explorer/OpenCode/brave/Notepad 等历次运行日志佐证）。

需手工物理验证（SendInput 无法模拟提权上下文中的输入法切换）：
1. 运行 `register_tsf.ps1` 注册；
2. 以管理员身份运行 CMD 或记事本；
3. 语言栏切换至 "Voime 语音输入"；
4. 按住 PTT 说话，松开应上屏。
（物理 Win+Space 在提权窗口有效，属标准 TSF 行为；自动化模拟按键在提权
上下文不进系统切换器，故未自动化。）

## 6. QA 汇总（全绿）

| 测试 | 结果 | 覆盖 |
| --- | --- | --- |
| 控制台自测 | 15/15 PASS | CPP-01~08 回归（NOTSF） |
| qa01_fullchain | PASS（263ms） | NOTSF 全链路回归 |
| qa02_guard | PASS | 60s 护栏 + 恢复性 |
| qa05_ipc | PASS | P5-02/03/04/05 IPC 协议全链路 + 重连 + 状态机拒绝 |
| qa06_tsf | PASS（312ms） | P5-01/04/05 TSF 注册 + 语言栏激活 + 按键拦截 + 注入 |
| qa07_uac | PASS | P5-07 自动化部分（SECUREMODE + 提权跨 IL 全会话） |

## 7. 已知限制与后续（阶段六衔接）

- 仅 x64 进程注入；32 位应用需后续构建 x86 TIP（或 WOW64 桥）。
- PTT 热键需避开全局占用组合（见 §4.9）；后续可加冲突检测提示。
- 跨用户管道 ACL 拒连、异常场景（录音中拔麦）的 TSF 模式回归，纳入 P6-04。
- Inno Setup 安装/卸载（P6-01/03）将封装 register/unregister 逻辑；
  卸载需处理已加载 DLL 的常驻进程（重启后清理策略，PRD 5.2）。
