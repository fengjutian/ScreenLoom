#!/usr/bin/env python3
"""一次性改名脚本：项目标识符 screenloom -> screenloom。

分层规则（顺序敏感）：
  A. 微信外部二进制里的真实符号 -> 文档别名 wx:: / WxShot*
     （字面量本身是微信 DLL 的事实，不能改；但项目里不应再出现该字样）
  B. 项目自身标识符 -> screenloom*
  C. 兜底：剩余裸 screenloom（按文件角色分别落到 screenloom 或 wx）

保持 BOM 与 LF 换行不变。
"""
import os
import re
import sys

ROOT = r"D:\github\screenloom"
SKIP_DIRS = {"target"}
EXTS = {".rs", ".toml", ".md", ".ps1", ".py", ".txt", ".json"}

# 这些文件里的裸 screenloom 一律指「微信原版命名空间」-> wx
EXTERNAL_FILES = {
    os.path.join(ROOT, "doc", "WeChat4-截图工具逆向规格书.md"),
    os.path.join(ROOT, "doc", "scripts", "scan_wechat_shot.py"),
}

# --- 通用有序规则 ---
RULES = [
    # exe / 进程名
    ("screenloom.exe", "screenloom.exe"),
    ("Get-Process -Name 'screenloom'", "Get-Process -Name 'screenloom'"),
    ('Get-Process -Name "screenloom"', 'Get-Process -Name "screenloom"'),
    ("Get-Process screenloom", "Get-Process screenloom"),
    # 绝对路径：旧仓库根 -> 当前根
    (r"D:\github\screenloom", r"D:\github\screenloom"),
    (r"D:\github\screenloom\doc", r"D:\github\screenloom\doc"),
    # 相对路径片段
    ("screenloom", "screenloom"),
    ("doc", "doc"),
    ("scan_wechat_shot.py", "scan_wechat_shot.py"),
    # Rust 标识符 / crate 名
    ("screenloom_core", "screenloom_core"),
    ("screenloom_capture", "screenloom_capture"),
    ("screenloom_ui", "screenloom_ui"),
    ("screenloom_ocr_", "screenloom_ocr_"),
    ("screenloom-app", "screenloom-app"),
    ("screenloom-capture", "screenloom-capture"),
    ("screenloom-core", "screenloom-core"),
    ("screenloom-ui", "screenloom-ui"),
    # 图标资源
    ("screenloom.ico", "screenloom.ico"),
    ("screenloom.png", "screenloom.png"),
    ("screenloom_32.png", "screenloom_32.png"),
    # 环境变量 / 大写标记
    ("SCREENLOOM", "SCREENLOOM"),
    # 窗口类名 / Pictures 目录等 PascalCase
    ("Screenloom", "Screenloom"),
    # 微信外部命名空间 -> 文档别名（全局：项目自身从不使用 wx:: ）
    ("wx::", "wx::"),
    ("WxShot", "WxShot"),
    # 兜底：下划线形态
    ("screenloom_", "screenloom_"),
]


def iter_files():
    for dirpath, dirnames, filenames in os.walk(ROOT):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for fn in filenames:
            ext = os.path.splitext(fn)[1].lower()
            if ext in EXTS or ext == ".log":
                yield os.path.join(dirpath, fn)


def main():
    changed = []
    for path in iter_files():
        rel = os.path.relpath(path, ROOT)
        with open(path, "rb") as f:
            raw = f.read()
        has_bom = raw.startswith(b"\xef\xbb\xbf")
        try:
            text = raw.decode("utf-8-sig")
        except UnicodeDecodeError:
            print(f"SKIP(non-utf8): {rel}")
            continue
        orig = text
        for a, b in RULES:
            text = text.replace(a, b)
        # 裸 screenloom 兜底：外部文件 -> wx，其余 -> screenloom
        if path in EXTERNAL_FILES:
            text = re.sub(r"\bweshot\b", "wx", text)
        else:
            text = re.sub(r"\bweshot\b", "screenloom", text)
        if text == orig:
            continue
        out = text.encode("utf-8")
        if has_bom:
            out = b"\xef\xbb\xbf" + out
        with open(path, "wb") as f:
            f.write(out)
        changed.append(rel)
    print(f"changed {len(changed)} files")
    for c in changed:
        print("  " + c)
    return 0


if __name__ == "__main__":
    sys.exit(main())
