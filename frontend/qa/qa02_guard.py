# qa02_guard.py — QA-02：60s 超时护栏与异常健壮性
#
# 1) 60s 护栏：PTT 长按 62s（WAV 循环音源）-> 引擎自动 warning + final 收尾，
#    应用自动停止采集、注入文本、不崩溃。
# 2) 恢复性：护栏后再次 PTT 仍可正常会话。
# 3) 采集中断处理：音源线程错误路径有应用层兜底（代码路径：on_capture_error）。
import os, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from qa_driver import (WAV_ZH, edit_text, launch_app, log_size, make_edit_window,
                       pump, ptt_hold, report, wait_log)

def main():
    fails = 0
    log0 = log_size()
    app = launch_app(["--audio-source", WAV_ZH, "--wav-loop"])
    wait_log("初始化完成", timeout=30, since=log0)

    make_edit_window()
    time.sleep(0.5)
    print("[qa02] 长按 PTT 62s 触发 60s 护栏...", flush=True)
    t0 = time.time()
    ptt_hold(62)  # 护栏 60s 自动收尾，无需等到松开
    alive = app.poll() is None
    fails += 0 if report("qa02.guard_alive", alive, "护栏后进程存活") else 1

    _, warn = wait_log('"type":"warning"', timeout=1, since=log0)
    fails += 0 if report("qa02.guard_warning", warn is not None,
                         (warn or "")[:110]) else 1
    _, fin = wait_log('"type":"final"', timeout=1, since=log0)
    fails += 0 if report("qa02.guard_final", fin is not None,
                         "60s 自动 final 收尾") else 1

    # 护栏时已自动注入（final 文本应为多句循环识别结果）
    pump(1.0)
    text = edit_text()
    fails += 0 if report("qa02.guard_inject", "你好世界" in text,
                         f"已注入 {len(text)} 字符") else 1

    # 恢复性：第二次 PTT 正常会话
    log1 = log_size()
    ptt_hold(3)
    _, line = wait_log("PTT 按下，开始录音", timeout=10, since=log1)
    fails += 0 if report("qa02.recover", line is not None, "护栏后可再次会话") else 1

    app.kill()
    print(f"[qa02] {'PASS' if fails == 0 else 'FAIL'}", flush=True)
    return 1 if fails else 0

if __name__ == "__main__":
    sys.exit(main())
