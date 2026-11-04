#!/usr/bin/env python3
"""
二进制字符串扫描器（修复版）。

## 这个脚本修的是什么

原版 `scan_binaries` 在 PowerShell 下这样跑：

    python scan_binaries.py ... | Select-Object -First 200

会以退出码 **4294967295 (0xFFFFFFFF / -1)** 失败，且输出在扫描中途被截断。

**根因不是内存不足，而是管道截断**：
`Select-Object -First N` 取够 N 行后会关闭管道，上游 python 被强制终止，
PowerShell 把「被中止的管道」上报为退出码 -1。

对照实验（可复现）：

    python -c "for i in range(200000): print('line', i)" | Select-Object -First 5 | Out-Null
    $LASTEXITCODE   # -> -1

    python -c "for i in range(200000): print('line', i)" | Out-File pipe_test.txt
    $LASTEXITCODE   # -> 0

## 本脚本的修法

1. **报告写文件，不靠管道**：stdout 只输出每文件一行摘要，不会被截断；
   详细命中与样本写进报告文件。管道怎么接都不会杀掉进程。
2. **mmap 分块读取**：不把整个文件读进内存（最大的 RadiumWMPF.bin 有 412MB），
   8MB 一块、块间 512B 重叠防止跨界字符串漏检。
3. **单文件失败隔离**：某个文件出错只记一条错误，继续扫下一个，不会整轮崩掉。
4. **样本数量有上限**：每个关键词最多留 N 条样本，输出量可控。

## 用法

    python scan_binaries.py [目录] [-o 报告路径] [--min-mb 0.5] [--samples 8] [--quiet]

退出码：0 = 全部完成；1 = 有文件级错误（已在报告中列出）。
"""
import argparse
import mmap
import os
import re
import sys
import time

# 关键词 -> 分组名
KEYS = [
    "screenshot", "screenshott", "screen_shot", "shot_ocr",
    "capture", "grab", "snip", "clipboard",
    "bitmap", "printwindow", "wgc", "dxgi", "gdi",
    "opencv", "longscreenshot",
]

ASCII_RE = re.compile(rb"[ -~]{5,}")
U16_RE = re.compile(rb"(?:[ -~]\x00){5,}")

CHUNK = 8 << 20        # 8MB
OVERLAP = 512          # 块间重叠，防跨界字符串漏检
CJK = "截图".encode("utf-8")


def scan_file(path, max_samples):
    """扫描单个文件，返回 (hits: dict, samples: dict, cjk_count, error)。"""
    hits = {}
    samples = {}
    cjk = 0
    size = os.path.getsize(path)
    try:
        with open(path, "rb") as f:
            mm = mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
            try:
                # ---- ASCII 扫描：分块 + 重叠 ----
                tail = b""
                pos = 0
                while pos < size:
                    chunk = mm[pos:pos + CHUNK]
                    if not chunk:
                        break
                    buf = tail + chunk
                    base = pos - len(tail)
                    for m in ASCII_RE.finditer(buf):
                        s = m.group()
                        low = s.lower()
                        for k in KEYS:
                            if k.encode() in low:
                                hits[k] = hits.get(k, 0) + 1
                                lst = samples.setdefault(k, [])
                                if len(lst) < max_samples:
                                    lst.append((base + m.start(),
                                                s.decode("ascii", "replace")[:150]))
                    tail = buf[-OVERLAP:] if len(buf) >= OVERLAP else buf
                    pos += CHUNK

                # ---- UTF-16LE 扫描：同样分块 ----
                tail = b""
                pos = 0
                while pos < size:
                    chunk = mm[pos:pos + CHUNK]
                    if not chunk:
                        break
                    buf = tail + chunk
                    base = pos - len(tail)
                    for m in U16_RE.finditer(buf):
                        try:
                            s = m.group().decode("utf-16-le")
                        except Exception:
                            continue
                        low = s.lower()
                        for k in KEYS:
                            if k in low:
                                key = "U16:" + k
                                hits[key] = hits.get(key, 0) + 1
                                lst = samples.setdefault(key, [])
                                if len(lst) < max_samples:
                                    lst.append((base + m.start(), s[:150]))
                    tail = buf[-OVERLAP:] if len(buf) >= OVERLAP else buf
                    pos += CHUNK

                # ---- CJK「截图」计数：同样分块，避免 mm[:] 整文件拷贝 ----
                cjk = 0
                tail = b""
                pos = 0
                while pos < size:
                    chunk = mm[pos:pos + CHUNK]
                    if not chunk:
                        break
                    buf = tail + chunk
                    cjk += buf.count(CJK)
                    tail = buf[-(len(CJK) - 1):] if len(CJK) > 1 else b""
                    pos += CHUNK
            finally:
                mm.close()
    except Exception as e:  # noqa: BLE001 - 单文件失败隔离
        return hits, samples, cjk, f"{type(e).__name__}: {e}"
    return hits, samples, cjk, None


def iter_files(root, min_size):
    for dirpath, _dirnames, filenames in os.walk(root):
        for fn in sorted(filenames):
            p = os.path.join(dirpath, fn)
            try:
                if os.path.getsize(p) >= min_size:
                    yield p
            except OSError:
                continue


def main():
    ap = argparse.ArgumentParser(description="二进制字符串扫描器（mmap 分块 + 报告写文件）")
    ap.add_argument("root", nargs="?", default=r"D:\TencentWeixin\Weixin\4.1.15.13",
                    help="待扫描目录")
    ap.add_argument("-o", "--out", default=None, help="报告输出路径（默认写到脚本同目录）")
    ap.add_argument("--min-mb", type=float, default=0.5, help="跳过小于该体积的文件")
    ap.add_argument("--samples", type=int, default=8, help="每个关键词最多保留的样本数")
    ap.add_argument("--quiet", action="store_true", help="不打印摘要表")
    args = ap.parse_args()

    root = args.root
    if not os.path.isdir(root):
        print(f"目录不存在: {root}", file=sys.stderr)
        return 1

    out_path = args.out or os.path.join(
        os.path.dirname(os.path.abspath(__file__)), "binaries_report.txt"
    )
    min_size = int(args.min_mb * 1024 * 1024)

    files = list(iter_files(root, min_size))
    t0 = time.time()
    errors = []

    with open(out_path, "w", encoding="utf-8") as rep:
        rep.write(f"# 二进制字符串扫描报告\n")
        rep.write(f"# 目录: {root}\n")
        rep.write(f"# 时间: {time.strftime('%Y-%m-%d %H:%M:%S')}\n")
        rep.write(f"# 文件数: {len(files)}   扫描耗时: 见末尾\n\n")

        for p in files:
            hits, samples, cjk, err = scan_file(p, args.samples)
            rel = os.path.relpath(p, root)
            size = os.path.getsize(p)
            if err:
                errors.append((rel, err))
                rep.write(f"### {rel}  ({size/1024/1024:.1f} MB)  ** 扫描失败: {err} **\n\n")
                continue
            if not hits and cjk <= 0:
                continue
            rep.write("=" * 90 + "\n")
            rep.write(f"### {rel}  ({size/1024/1024:.1f} MB)\n")
            for k in sorted(hits, key=lambda x: -hits[x]):
                rep.write(f"  {k:22s} {hits[k]}\n")
            if cjk > 0:
                rep.write(f"  {'CJK_截图':22s} {cjk}\n")
            for k in sorted(samples):
                rep.write(f"  -- {k}:\n")
                for off, s in samples[k]:
                    rep.write(f"     @{off:>11} {s}\n")
            rep.write("\n")

        rep.write("=" * 90 + "\n")
        rep.write(f"扫描完成: {len(files)} 个文件, 耗时 {time.time()-t0:.1f}s, 错误 {len(errors)} 个\n")
        for rel, err in errors:
            rep.write(f"  ! {rel}: {err}\n")

    if not args.quiet:
        # 即使调用方仍然用 `| Select-Object -First N` 截断管道，也不该以失败告终：
        # 报告已经落盘，stdout 只是给人看的摘要。
        try:
            print(f"扫描 {len(files)} 个文件, 耗时 {time.time()-t0:.1f}s, 错误 {len(errors)} 个")
            print(f"报告: {out_path}")
            for rel, err in errors:
                print(f"  ! {rel}: {err}")
            sys.stdout.flush()
        except BrokenPipeError:
            # 管道被关闭：把 stdout 指向 devnull，避免解释器退出时的二次报错
            try:
                devnull = os.open(os.devnull, os.O_WRONLY)
                os.dup2(devnull, sys.stdout.fileno())
            except OSError:
                pass
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
