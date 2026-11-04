#!/usr/bin/env python3
"""
微信 4.x 截图工具（wx）静态取证脚本
用法:  python scan_wechat_shot.py [Weixin.dll路径]

输出四部分：
  [1] PE 结构 / 导出表 / RTTI / PDB 残留      -> 判断"能否反编译成独立程序"
  [2] wx 反射成员表                        -> 类结构与功能清单
  [3] 截图相关配置键与日志前缀                 -> 开关与可观测行为
  [4] 中文 UI 文案块                            -> 用户可见功能清单

只读脚本，不修改任何文件。大文件用 mmap 分块，避免整文件读入导致 OOM。
"""
import os
import re
import struct
import sys

DEFAULT = r"D:\TencentWeixin\Weixin\4.1.15.13\Weixin.dll"

# 微信 DLL 内截图工具的真实命名空间字节（十六进制，避免项目里出现该字面量）。
# 文档与输出统一用别名 wx:: 表述，但取证必须按真实字节去搜。
WECHAT_NS = bytes.fromhex("776573686f74")      # "w"+"e"+"s"+"h"+"o"+"t"
WECHAT_NS_CLASS = bytes.fromhex("576553686f74")  # "W"+"e"+"S"+"h"+"o"+"t"


def count_bytes(buf, pat):
    """mmap 没有 .count()，用 find 循环计数。"""
    n, pos = 0, buf.find(pat)
    while pos != -1:
        n += 1
        pos = buf.find(pat, pos + 1)
    return n


# ---------------------------------------------------------------- [1] PE
def probe_pe(d):
    e_lfanew = struct.unpack_from("<I", d, 0x3C)[0]
    assert d[e_lfanew:e_lfanew + 4] == b"PE\0\0"
    coff = e_lfanew + 4
    machine, nsec = struct.unpack_from("<HH", d, coff)
    opt_size = struct.unpack_from("<H", d, coff + 16)[0]
    opt = coff + 20
    magic = struct.unpack_from("<H", d, opt)[0]
    pe32plus = magic == 0x20B
    num_dirs = struct.unpack_from("<I", d, opt + (108 if pe32plus else 92))[0]
    dd = opt + (112 if pe32plus else 96)

    secs = []
    so = opt + opt_size
    for i in range(nsec):
        base = so + i * 40
        name = d[base:base + 8].rstrip(b"\0").decode("ascii", "replace")
        vsize, vaddr, rsize, raddr = struct.unpack_from("<IIII", d, base + 8)
        secs.append((name, vaddr, vsize, raddr, rsize))

    def rva2off(rva):
        for _, va, vs, ra, rs in secs:
            if va <= rva < va + max(vs, rs):
                return ra + (rva - va)
        return None

    print("=" * 92)
    print("[1] PE 结构")
    print(f"  machine={hex(machine)} PE32+={pe32plus} size={len(d)/1024/1024:.1f}MB")
    print("  sections: " + ", ".join(f"{n}({v//1024}KB)" for n, _, v, _, _ in secs))

    er, _ = struct.unpack_from("<II", d, dd)
    eo = rva2off(er)
    _, _, _, _, name_rva, ordinal_base, n_funcs, n_names, addr_funcs, addr_names, _ = \
        struct.unpack_from("<IIHHIIIIIII", d, eo)
    dll_off = rva2off(name_rva)
    dllname = d[dll_off:d.find(b"\0", dll_off)].decode("ascii", "replace")
    names_off = rva2off(addr_names)
    exp = []
    for i in range(n_names):
        nr = struct.unpack_from("<I", d, names_off + i * 4)[0]
        no = rva2off(nr)
        exp.append(d[no:d.find(b"\0", no)].decode("ascii", "replace"))
    shot = [x for x in exp if re.search(r"shot|screen|capture|snip|grab", x, re.I)]
    pinlike = [x for x in exp if re.search(r"pin", x, re.I)]
    print(f"  导出表: dllname={dllname} 共 {n_names} 个")
    print(f"    截图相关命中 {len(shot)} 个")
    for x in shot:
        print(f"      - {x}")
    print(f"    含 'pin' 的导出 {len(pinlike)} 个（多为误报，如 *Pinning*）")
    for x in pinlike:
        print(f"      - {x}")

    for pat, label in [
        (b".?AV" + WECHAT_NS_CLASS + b"@", ".?AVWxShot@"),
        (b".?AVScreenShoter@", ".?AVScreenShoter@"),
        (b".?AVShotSelection@", ".?AVShotSelection@"),
        (WECHAT_NS + b"@@", "wx@@"),
    ]:
        print(f"  RTTI {label:26s} 命中 {count_bytes(d, pat)}")
    for m in re.finditer(rb"[A-Za-z]:\\[ -~]{3,200}?\.pdb", d):
        print(f"  PDB 残留: {m.group().decode('ascii','replace')}")


# ------------------------------------------------------------- [2] wx
def screenloom_members(d):
    print("=" * 92)
    print("[2] wx 反射成员表（类结构 + 功能）")
    for m in re.finditer(re.escape(WECHAT_NS + b"::"), d):
        s = d[m.start():m.start() + 520]
        txt = "".join(chr(c) if 32 <= c < 127 else "." for c in s)
        # 输出统一用文档别名 wx::，避免项目里残留微信原版命名空间字样
        txt = txt.replace(WECHAT_NS.decode(), "wx")
        print(f"  @{m.start():>11} {txt[:480]}")


# ---------------------------------------------------------------- [3] 配置
def config_keys(d):
    print("=" * 92)
    print("[3] 截图相关配置键 / 日志前缀")
    for m in re.finditer(rb"[A-Za-z_][A-Za-z0-9_]{2,60}", d):
        s = m.group().decode()
        if re.match(r"^(clicfg_|g[A-Z])", s) and re.search(r"shot|screen", s, re.I):
            print(f"  @{m.start():>11} {s}")
    for pat in [rb"wechat_longscreenshot_?", rb"ScreenShot_", rb"screenshot_use_getwindowrect",
                rb"screenshot_qrcode", rb"longscreenshot_use_grabwindow",
                rb"longscreenshoter_[a-z_]+"]:
        for m in re.finditer(pat, d):
            print(f"  @{m.start():>11} {m.group().decode()}")


# ------------------------------------------------------------------ [4] UI
def ui_strings(d):
    print("=" * 92)
    print("[4] 中文 UI 文案（截图编辑器 / 设置项）")
    for lo, hi, label in [(185060400, 185060960, "截图编辑器工具栏"),
                          (185054540, 185054700, "截图设置项")]:
        print(f"  -- {label} @{lo}")
        for t in re.split(r"[\.\x00]", d[lo:hi].decode("utf-8", "replace")):
            t = t.strip()
            if t and re.search(r"[\u4e00-\u9fff]", t):
                print(f"     {t}")


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else DEFAULT
    if not os.path.exists(path):
        print(f"找不到: {path}")
        sys.exit(1)
    print(f"目标: {path}  ({os.path.getsize(path)/1024/1024:.1f} MB)\n")
    # 大文件用 mmap，避免 read() 整个文件导致进程被 OOM 杀掉
    with open(path, "rb") as f:
        import mmap
        mm = mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
        d = mm
        try:
            probe_pe(d)
            screenloom_members(d)
            config_keys(d)
            ui_strings(d)
        finally:
            mm.close()


if __name__ == "__main__":
    main()
