# qa01_fullchain.py — QA-01：集成全链路测试
#
# 按下说话到文字输入全流程（产品二进制 + 真实 PTT 钩子 + WAV 音源）：
# PTT 按下 -> 采集(WavSource) -> 引擎 -> partial 流式 -> PTT 松开 -> 标点补全
# -> SendInput 注入目标窗口。断言：注入文本正确、松开到注入延迟 <500ms、
# 日志含完整会话事件序列。前置：模型已就位（qa03 已下载至 %APPDATA%）。
import os, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from qa_driver import (WAV_ZH, edit_text, focus_ok, launch_app, log_size,
                       make_edit_window, pump, ptt_hold, report, wait_log)

EXPECT = "你好世界，你好世界，你好世界。"

def main():
    fails = 0
    log0 = log_size()

    app = launch_app(["--audio-source", WAV_ZH])
    _, line = wait_log("初始化完成", timeout=30, since=log0)
    fails += 0 if report("qa01.startup", line is not None, "应用就绪") else 1

    make_edit_window()
    time.sleep(0.5)
    ok_focus = focus_ok()
    report("qa01.focus", ok_focus, "目标 EDIT 窗口聚焦")

    t0 = time.time()
    ptt_hold(9.5)  # wav 8.88s + 余量
    t_rel = time.time()

    # 等注入文本出现（轮询 EDIT 内容）—— 即端到端延迟
    text, latency = "", None
    end = time.time() + 15
    while time.time() < end:
        pump(0.05)
        text = edit_text()
        if text:
            latency = time.time() - t_rel
            break

    fails += 0 if report("qa01.inject_text", text == EXPECT, f"注入文本: {text!r}") else 1
    fails += 0 if report("qa01.latency", latency is not None and latency < 0.5,
                         f"松开->注入 {latency and round(latency * 1000)}ms (目标<500ms)") else 1

    # 日志事件序列完整性
    seq_ok = True
    for marker in ["会话开始", '"type":"partial"', "会话结束，进入 Finalizing",
                   '"type":"final"', "已注入"]:
        if wait_log(marker, timeout=1, since=log0)[0] is None:
            seq_ok = False
            print(f"    缺日志事件: {marker}", flush=True)
    fails += 0 if report("qa01.event_seq", seq_ok, "按下/partial/松开/final/注入 全序列") else 1

    app.kill()
    print(f"[qa01] {'PASS' if fails == 0 else 'FAIL'}", flush=True)
    return 1 if fails else 0

if __name__ == "__main__":
    sys.exit(main())
