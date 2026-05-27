"""Simulate a frontend client for testing the ASR WebSocket server.

Usage:
    python tests/simulate_client.py                     # record from mic
    python tests/simulate_client.py --file sample.wav   # send a WAV file
"""

import argparse
import asyncio
import json
import sys
import wave
from pathlib import Path

import aiohttp
import sounddevice as sd


async def stream_mic(ws, sample_rate: int = 16000, chunk_duration: float = 0.1):
    """Stream audio from microphone to the server."""
    chunk_size = int(sample_rate * chunk_duration)
    loop = asyncio.get_event_loop()
    queue = asyncio.Queue()

    def callback(indata, frames, time_info, status):
        if status:
            print(f"Audio status: {status}", file=sys.stderr)
        asyncio.run_coroutine_threadsafe(
            queue.put(bytes(indata.tobytes())), loop
        )

    stream = sd.InputStream(
        samplerate=sample_rate, channels=1, dtype="int16",
        blocksize=chunk_size, callback=callback,
    )

    print("Recording... (Ctrl+C to stop)")
    stream.start()

    await ws.send_json({"type": "speech_start"})

    try:
        while True:
            chunk = await queue.get()
            await ws.send_bytes(chunk)
    except asyncio.CancelledError:
        pass
    finally:
        stream.stop()
        stream.close()
        await ws.send_json({"type": "speech_end"})


async def stream_file(ws, filepath: str, sample_rate: int = 16000, chunk_duration: float = 0.1):
    """Stream a WAV file to the server."""
    chunk_size = int(sample_rate * chunk_duration * 2)  # 2 bytes per sample

    with wave.open(filepath, "rb") as wf:
        print(f"File: {wf.getnchannels()}ch, {wf.getframerate()}Hz, {wf.getnframes()} frames")

        await ws.send_json({
            "type": "config",
            "sample_rate": wf.getframerate(),
            "language": "zh",
            "model": "base"
        })

        await ws.send_json({"type": "speech_start"})

        while True:
            data = wf.readframes(chunk_size // 2)
            if not data:
                break
            await ws.send_bytes(data)
            await asyncio.sleep(chunk_duration)

        await ws.send_json({"type": "speech_end"})


async def main():
    parser = argparse.ArgumentParser(description="Voime ASR test client")
    parser.add_argument("--file", help="WAV file to stream")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=9876)
    parser.add_argument("--language", default="zh")
    parser.add_argument("--polish", action="store_true", help="Request AI polishing after final")
    args = parser.parse_args()

    url = f"ws://{args.host}:{args.port}/asr"
    print(f"Connecting to {url}...")

    async with aiohttp.ClientSession() as session:
        async with session.ws_connect(url) as ws:
            await ws.send_json({
                "type": "config",
                "language": args.language,
                "model": "base"
            })
            msg = await ws.receive_json()
            print(f"Server: {msg}")

            receive_task = asyncio.create_task(read_responses(ws))

            if args.file:
                await stream_file(ws, args.file)
            else:
                await stream_mic(ws)

            # Wait for final responses
            await asyncio.sleep(2)

            if args.polish:
                await ws.send_json({
                    "type": "polish",
                    "text": "我今天想去吃个苹果"
                })

            await asyncio.sleep(2)
            receive_task.cancel()

            await ws.close()


async def read_responses(ws):
    """Print server responses."""
    try:
        async for msg in ws:
            if msg.type == aiohttp.WSMsgType.TEXT:
                data = json.loads(msg.data)
                tag = data.get("type", "?")
                text = data.get("text", "") or data.get("message", "")
                stable = "✓" if data.get("stable") else " "
                print(f"  [{tag}] [{stable}] {text}")
            elif msg.type == aiohttp.WSMsgType.CLOSED:
                break
    except asyncio.CancelledError:
        pass


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        print("\nStopped.")
