# qa05_ipc.py — 阶段五 P5-02/P5-03/P5-04(协议侧)/P5-05(提交侧) IPC 全链路验收
#
# 标准版 Agent（TSF/IPC 模式，无热键/SendInput）+ WAV 音源：
# python 模拟 TSF 客户端连 Named Pipe，发 start/stop_session 驱动引擎状态机，
# 断言：应答 code、commit_text 文本正确、录音中重复 start 被拒、断线重连。
# 前置：模型已就位（qa03 已下载至 %APPDATA%）。
import json, os, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from qa_driver import (APP_STD, PipeClient, WAV_ZH, launch_app, log_size, report,
                       wait_log)

EXPECT = "你好世界，你好世界，你好世界。"

def req_id(n):
    return f"qa05-{n:04d}-0000-4000-8000-000000000000"

def main():
    fails = 0
    log0 = log_size()

    app = launch_app(["--audio-source", WAV_ZH], exe=APP_STD)
    try:
        fails = run(log0)
    finally:
        app.kill()
    print(f"[qa05] {'PASS' if fails == 0 else 'FAIL'}", flush=True)
    return 1 if fails else 0

def run(log0):
    fails = 0

    _, line = wait_log("IPC 服务已监听", timeout=30, since=log0)
    fails += 0 if report("qa05.ipc_listen", line is not None,
                         "Agent 管道进入监听") else 1

    # --- 连接 + 全链路：start -> 识别 -> stop -> commit_text ---
    print("[qa05] 连接管道...", flush=True)
    pipe = PipeClient()
    fails += 0 if report("qa05.connect", pipe.connect(), "管道连接成功") else 1
    _, line = wait_log("Client Connected", timeout=5, since=log0)
    fails += 0 if report("qa05.server_log", line is not None,
                         (line or "").strip()[-48:]) else 1

    print("[qa05] start_session...", flush=True)
    pipe.send_json({"action": "start_session", "request_id": req_id(1)})
    resp = pipe.recv_json(timeout=10)
    ok = resp.get("request_id") == req_id(1) and resp.get("code") == 0
    fails += 0 if report("qa05.start_resp", ok, f"start 应答: {resp}") else 1

    print("[qa05] 会话中（WAV 8.88s）...", flush=True)
    time.sleep(9.5)
    pipe.send_json({"action": "stop_session", "request_id": req_id(2)})
    resp = pipe.recv_json(timeout=10)
    ok = resp.get("request_id") == req_id(2) and resp.get("code") == 0
    fails += 0 if report("qa05.stop_resp", ok, f"stop 应答: {resp}") else 1

    commit = pipe.recv_json(timeout=20)
    ok = commit.get("action") == "commit_text" and commit.get("payload") == EXPECT
    fails += 0 if report("qa05.commit_text", ok,
                         f"commit_text: {commit.get('payload')!r}") else 1
    # 模拟 TSF 注入成功应答
    pipe.send_json({"request_id": commit.get("request_id", ""),
                    "status": "success", "code": 0})

    # --- 录音中重复 start 应被拒绝（引擎状态机） ---
    pipe.send_json({"action": "start_session", "request_id": req_id(3)})
    resp = pipe.recv_json(timeout=10)
    ok1 = resp.get("code") == 0
    pipe.send_json({"action": "start_session", "request_id": req_id(4)})
    resp2 = pipe.recv_json(timeout=10)
    ok2 = resp2.get("code") != 0
    fails += 0 if report("qa05.dup_start", ok1 and ok2,
                         f"二次 start code={resp2.get('code')}（非 0 拒绝）") else 1
    pipe.send_json({"action": "stop_session", "request_id": req_id(5)})
    pipe.recv_json(timeout=10)
    try:
        pipe.recv_json(timeout=20)  # 丢弃本次 commit_text
    except Exception:
        pass

    # --- 断线重连（P5-02 容错：Agent 继续监听，客户端可重连） ---
    pipe.close()
    time.sleep(0.5)
    pipe2 = PipeClient()
    ok = pipe2.connect()
    fails += 0 if report("qa05.reconnect", ok, "断开后重连成功") else 1
    if ok:
        _, line = wait_log("Client Connected", timeout=5,
                           since=log_size() - 5)
        pipe2.send_json({"action": "start_session", "request_id": req_id(6)})
        resp = pipe2.recv_json(timeout=10)
        fails += 0 if report("qa05.reconnect_session", resp.get("code") == 0,
                             "重连后会话可用") else 1
        pipe2.send_json({"action": "stop_session", "request_id": req_id(7)})
        pipe2.recv_json(timeout=10)
        try:
            pipe2.recv_json(timeout=20)
        except Exception:
            pass
        pipe2.close()
    return fails

if __name__ == "__main__":
    sys.exit(main())
