# qa07_elevated_client.py — 提权运行的全 IPC 会话（由 qa07 以 RunAs 启动）
# 与 Agent（中完整性）跨 IL 走完整 start->stop->commit 流程，结果写文件。
import os, sys, time
sys.path.insert(0, r"D:\Work_win\voime\frontend\qa")
from qa_driver import PipeClient

OUT = os.path.expandvars(r"%TEMP%\voime_qa_sandbox\qa07_elevated_result.txt")
EXPECT = "你好世界，你好世界，你好世界。"

def main():
    p = PipeClient()
    if not p.connect(timeout=15):
        open(OUT, "w", encoding="utf-8").write("FAIL connect")
        return 1
    p.send_json({"action": "start_session", "request_id": "qa07-e-1"})
    r1 = p.recv_json(timeout=10)
    time.sleep(9.5)
    p.send_json({"action": "stop_session", "request_id": "qa07-e-2"})
    r2 = p.recv_json(timeout=10)
    commit = p.recv_json(timeout=25)
    ok = (r1.get("code") == 0 and r2.get("code") == 0 and
          commit.get("action") == "commit_text" and commit.get("payload") == EXPECT)
    if commit.get("action") == "commit_text":
        p.send_json({"request_id": commit.get("request_id", ""),
                     "status": "success", "code": 0})
    p.close()
    open(OUT, "w", encoding="utf-8").write(
        f"{'PASS' if ok else 'FAIL'} r1={r1.get('code')} r2={r2.get('code')} "
        f"payload={commit.get('payload')!r}")
    return 0 if ok else 1

if __name__ == "__main__":
    sys.exit(main())
