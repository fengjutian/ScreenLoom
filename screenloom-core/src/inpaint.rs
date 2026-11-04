//! 内容填充（消除杂物）。
//!
//! 微信的「消除杂物」是 AI inpainting；本项目**没有可用的 AI inpainting 端点**
//! （MiniMax image-01 / DeepSeek 都没有 mask/inpainting 字段），所以这里实现的是
//! **本地多尺度样例填充**（exemplar-based content-aware fill，PatchMatch 风格）：
//!
//! 1. 洞（待填充区域）按 mask 处理，图像按 2x box 滤波建金字塔；
//! 2. 由粗到细逐层处理：先扩散初始化洞，再在**已知区域**里搜索最佳匹配 patch
//!    （SSD，只比较 patch 中已知的像素），把最佳匹配 patch 的对应像素投票写回洞；
//! 3. 最细层结果写回原图，**只写 region 之内**。
//!
//! 整个过程是确定性的（固定种子的 xorshift，不依赖系统时间），也不会 panic。

use crate::geometry::Rect;
use crate::image::{Rgba, RgbaImage};

/// 内容填充参数。
#[derive(Clone, Copy, Debug)]
pub struct InpaintOptions {
    /// 金字塔层数（0 = 不缩放，只做单尺度）。默认 3。
    pub pyramid_levels: u32,
    /// 匹配用的 patch 边长（像素）。默认 7。
    pub patch_size: u32,
    /// 每层迭代次数。默认 3。
    pub iterations: u32,
}

impl Default for InpaintOptions {
    fn default() -> Self {
        Self {
            pyramid_levels: 3,
            patch_size: 7,
            iterations: 3,
        }
    }
}

/// 随机候选采样次数（每个洞像素每次迭代）。
const RANDOM_CANDIDATES: usize = 4;
/// 洞在金字塔中小于该尺寸就不再继续下采样（像素）。
const SMALL_HOLE: usize = 16;
/// 扩散初始化的最大轮数（防止超大洞把时间吃光）。
const MAX_DIFFUSE_ROUNDS: usize = 256;

/// 把 `region` 矩形内的像素用周围纹理补上（消除杂物）。
///
/// 返回**新的**图像；输入图像不被修改；`region` 之外的像素与输入**逐字节相同**。
pub fn inpaint_region(img: &RgbaImage, region: Rect) -> RgbaImage {
    inpaint_region_with(img, region, &InpaintOptions::default())
}

/// 同 [`inpaint_region`]，可传参数。
pub fn inpaint_region_with(img: &RgbaImage, region: Rect, opts: &InpaintOptions) -> RgbaImage {
    let mut out = img.clone();

    // ---- 退化输入：原样返回副本，绝不 panic ----
    if img.w == 0 || img.h == 0 {
        return out;
    }
    let r = region.normalized();
    if r.is_empty() {
        return out;
    }
    let x0 = r.x.max(0);
    let y0 = r.y.max(0);
    let x1 = r.right().min(img.w as i32);
    let y1 = r.bottom().min(img.h as i32);
    if x1 <= x0 || y1 <= y0 {
        // 完全在图外
        return out;
    }
    if x1 - x0 >= img.w as i32 && y1 - y0 >= img.h as i32 {
        // 覆盖整图：没有任何已知像素可供采样
        return out;
    }

    let hole = Rect::new(x0, y0, x1 - x0, y1 - y0);
    let ps = opts.patch_size.max(3) as usize;

    // ---- 建金字塔 ----
    let levels = build_pyramid(img, hole, opts.pyramid_levels as usize, ps);

    // ---- 由粗到细逐层处理 ----
    let mut rng = Rng::new(0x9E37_79B9_7F4A_7C15);
    let mut prev: Option<Level> = None;
    for mut lvl in levels.into_iter().rev() {
        match &prev {
            Some(coarse) => upsample_init(&mut lvl, coarse),
            None => diffusion_init(&mut lvl),
        }
        fill_level(&mut lvl, ps, opts.iterations, &mut rng);
        prev = Some(lvl);
    }

    // ---- 只把最细层的洞内容写回 ----
    if let Some(final_level) = prev {
        write_back(&mut out, &final_level);
    }
    out
}

// ---------------------------------------------------------------------------------------------
// 金字塔层级
// ---------------------------------------------------------------------------------------------

/// 一个金字塔层级：RGBA8 像素 + 洞掩膜。
struct Level {
    w: usize,
    h: usize,
    img: Vec<u8>,
    /// true = 待填充（洞）。
    hole: Vec<bool>,
    /// 洞的包围盒 `(x0, y0, x1, y1)`（左闭右开）。
    bbox: (usize, usize, usize, usize),
}

impl Level {
    fn from_image(img: &RgbaImage, hole: Rect) -> Level {
        let w = img.w as usize;
        let h = img.h as usize;
        let mut lv = Level {
            w,
            h,
            img: img.data.clone(),
            hole: vec![false; w * h],
            bbox: (0, 0, 0, 0),
        };
        let x0 = hole.x.max(0) as usize;
        let y0 = hole.y.max(0) as usize;
        let x1 = (hole.right().min(img.w as i32)).max(0) as usize;
        let y1 = (hole.bottom().min(img.h as i32)).max(0) as usize;
        for y in y0..y1.min(h) {
            for x in x0..x1.min(w) {
                lv.hole[y * w + x] = true;
            }
        }
        lv.recompute_bbox();
        lv
    }

    fn recompute_bbox(&mut self) {
        let mut x0 = self.w;
        let mut y0 = self.h;
        let mut x1 = 0usize;
        let mut y1 = 0usize;
        for y in 0..self.h {
            for x in 0..self.w {
                if self.hole[y * self.w + x] {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        if x1 <= x0 {
            self.bbox = (0, 0, 0, 0);
        } else {
            self.bbox = (x0, y0, x1, y1);
        }
    }

    /// 2x box 滤波下采样；掩膜用「任一子像素为洞即为洞」的保守规则。
    fn downsample(&self) -> Level {
        let nw = (self.w + 1) / 2;
        let nh = (self.h + 1) / 2;
        let mut out = Level {
            w: nw,
            h: nh,
            img: vec![0u8; nw * nh * 4],
            hole: vec![false; nw * nh],
            bbox: (0, 0, 0, 0),
        };
        for y in 0..nh {
            for x in 0..nw {
                let (mut r, mut g, mut b, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
                let mut any_hole = false;
                for dy in 0..2 {
                    for dx in 0..2 {
                        let sx = x * 2 + dx;
                        let sy = y * 2 + dy;
                        if sx >= self.w || sy >= self.h {
                            continue;
                        }
                        let si = sy * self.w + sx;
                        let p = si * 4;
                        r += self.img[p] as u32;
                        g += self.img[p + 1] as u32;
                        b += self.img[p + 2] as u32;
                        a += self.img[p + 3] as u32;
                        n += 1;
                        if self.hole[si] {
                            any_hole = true;
                        }
                    }
                }
                let di = y * nw + x;
                if n > 0 {
                    out.img[di * 4] = ((r + n / 2) / n) as u8;
                    out.img[di * 4 + 1] = ((g + n / 2) / n) as u8;
                    out.img[di * 4 + 2] = ((b + n / 2) / n) as u8;
                    out.img[di * 4 + 3] = ((a + n / 2) / n) as u8;
                }
                out.hole[di] = any_hole;
            }
        }
        out.recompute_bbox();
        out
    }
}

/// 建金字塔：level 0 = 原分辨率，之后每层 2x 下采样。
fn build_pyramid(img: &RgbaImage, hole: Rect, max_levels: usize, ps: usize) -> Vec<Level> {
    let mut levels = vec![Level::from_image(img, hole)];
    while levels.len() <= max_levels {
        let cur = levels.last().expect("levels 非空");
        let (bx0, by0, bx1, by1) = cur.bbox;
        if bx1 <= bx0 {
            break; // 没有洞
        }
        if (bx1 - bx0).max(by1 - by0) <= SMALL_HOLE {
            break; // 洞已经足够小
        }
        let nw = (cur.w + 1) / 2;
        let nh = (cur.h + 1) / 2;
        if nw < ps || nh < ps {
            break; // 再小就没有合法的 patch 源
        }
        let next = cur.downsample();
        levels.push(next);
    }
    levels
}

// ---------------------------------------------------------------------------------------------
// 初始化
// ---------------------------------------------------------------------------------------------

/// 从洞边界向内做各向同性扩散（多轮 4 邻域平均），给洞一个初值。
fn diffusion_init(lvl: &mut Level) {
    let w = lvl.w;
    let (bx0, by0, bx1, by1) = lvl.bbox;
    if bx1 <= bx0 || by1 <= by0 {
        return;
    }
    let rounds = (bx1 - bx0).max(by1 - by0).min(MAX_DIFFUSE_ROUNDS);
    for _ in 0..rounds {
        let mut changed = false;
        for y in by0..by1 {
            for x in bx0..bx1 {
                let idx = y * w + x;
                if !lvl.hole[idx] {
                    continue;
                }
                let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
                let neigh = [
                    (x as i32 - 1, y as i32),
                    (x as i32 + 1, y as i32),
                    (x as i32, y as i32 - 1),
                    (x as i32, y as i32 + 1),
                ];
                for (nx, ny) in neigh {
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= lvl.h as i32 {
                        continue;
                    }
                    let ni = ny as usize * w + nx as usize;
                    let p = ni * 4;
                    r += lvl.img[p] as u32;
                    g += lvl.img[p + 1] as u32;
                    b += lvl.img[p + 2] as u32;
                    n += 1;
                }
                if n == 0 {
                    continue;
                }
                let nr = ((r + n / 2) / n) as u8;
                let ng = ((g + n / 2) / n) as u8;
                let nb = ((b + n / 2) / n) as u8;
                if lvl.img[idx * 4] != nr
                    || lvl.img[idx * 4 + 1] != ng
                    || lvl.img[idx * 4 + 2] != nb
                {
                    changed = true;
                    lvl.img[idx * 4] = nr;
                    lvl.img[idx * 4 + 1] = ng;
                    lvl.img[idx * 4 + 2] = nb;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// 用粗层结果双线性上采样，初始化细层的洞。
fn upsample_init(fine: &mut Level, coarse: &Level) {
    let (fw, fh) = (fine.w, fine.h);
    let (cw, ch) = (coarse.w, coarse.h);
    if fw == 0 || fh == 0 || cw == 0 || ch == 0 {
        return;
    }
    let xden = fw.saturating_sub(1).max(1) as i64;
    let yden = fh.saturating_sub(1).max(1) as i64;
    let xspan = cw as i64 - 1;
    let yspan = ch as i64 - 1;
    for y in 0..fh {
        let fy = y as i64 * yspan;
        let sy0 = (fy / yden) as usize;
        let sy1 = (sy0 + 1).min(ch - 1);
        let wy = fy - sy0 as i64 * yden;
        for x in 0..fw {
            if !fine.hole[y * fw + x] {
                continue;
            }
            let fx = x as i64 * xspan;
            let sx0 = (fx / xden) as usize;
            let sx1 = (sx0 + 1).min(cw - 1);
            let wx = fx - sx0 as i64 * xden;
            let di = (y * fw + x) * 4;
            for c in 0..4 {
                let p00 = coarse.img[(sy0 * cw + sx0) * 4 + c] as i64;
                let p01 = coarse.img[(sy0 * cw + sx1) * 4 + c] as i64;
                let p10 = coarse.img[(sy1 * cw + sx0) * 4 + c] as i64;
                let p11 = coarse.img[(sy1 * cw + sx1) * 4 + c] as i64;
                let top = p00 * (xden - wx) + p01 * wx;
                let bot = p10 * (xden - wx) + p11 * wx;
                fine.img[di + c] = ((top * (yden - wy) + bot * wy) / (xden * yden)) as u8;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 样例填充
// ---------------------------------------------------------------------------------------------

/// 一个候选 patch（源中心像素）。
#[derive(Clone, Copy)]
struct Cand {
    ssd: u64,
    base: usize,
}

/// 计算候选 patch 与目标 patch 的 SSD（只比较目标 patch 中「已知」的像素）。
#[inline]
fn patch_ssd(img: &[u8], base: usize, offs: &[i32], vals: &[[u8; 3]]) -> u64 {
    let mut sum = 0u64;
    for (k, off) in offs.iter().enumerate() {
        let sp = (base as i64 + *off as i64) as usize;
        let p = sp * 4;
        let d0 = img[p] as i32 - vals[k][0] as i32;
        let d1 = img[p + 1] as i32 - vals[k][1] as i32;
        let d2 = img[p + 2] as i32 - vals[k][2] as i32;
        sum += (d0 * d0 + d1 * d1 + d2 * d2) as u64;
    }
    sum
}

/// 合法源中心：其 `2*half+1` 的 patch 必须完整落在图内，且与洞不相交。
fn valid_source_mask(lvl: &Level, half: i32) -> Vec<bool> {
    let w = lvl.w;
    let h = lvl.h;
    // 水平膨胀（可分离，运行计数 O(N)）
    let mut tmp = vec![false; w * h];
    for y in 0..h {
        let mut cnt = 0u32;
        let upto = half.max(0).min(w as i32 - 1);
        for x in 0..=upto as usize {
            if lvl.hole[y * w + x] {
                cnt += 1;
            }
        }
        for x in 0..w {
            tmp[y * w + x] = cnt > 0;
            let out = x as i32 - half;
            if out >= 0 && lvl.hole[y * w + out as usize] {
                cnt -= 1;
            }
            let inn = x as i32 + half + 1;
            if inn >= 0 && (inn as usize) < w && lvl.hole[y * w + inn as usize] {
                cnt += 1;
            }
        }
    }
    // 垂直膨胀 + 边界检查
    let mut out = vec![false; w * h];
    for x in 0..w {
        let mut cnt = 0u32;
        let upto = half.max(0).min(h as i32 - 1);
        for y in 0..=upto as usize {
            if tmp[y * w + x] {
                cnt += 1;
            }
        }
        for y in 0..h {
            let mut ok = cnt == 0;
            let xi = x as i32;
            let yi = y as i32;
            if xi < half || yi < half || xi >= w as i32 - half || yi >= h as i32 - half {
                ok = false; // patch 放不进图内
            }
            out[y * w + x] = ok;
            let o = yi - half;
            if o >= 0 && tmp[o as usize * w + x] {
                cnt -= 1;
            }
            let i = yi + half + 1;
            if i >= 0 && (i as usize) < h && tmp[i as usize * w + x] {
                cnt += 1;
            }
        }
    }
    out
}

/// 在这一层里把洞填上：每个洞像素在已知区域里搜索最佳匹配 patch，投票写回。
fn fill_level(lvl: &mut Level, ps: usize, iterations: u32, rng: &mut Rng) {
    let w = lvl.w;
    let h = lvl.h;
    if w == 0 || h == 0 {
        return;
    }
    let half = (ps / 2).max(1) as i32;
    let valid = valid_source_mask(lvl, half);
    if !valid.iter().any(|&v| v) {
        return; // 没有合法源，保留初值
    }

    let mut known: Vec<bool> = lvl.hole.iter().map(|&b| !b).collect();
    let mut best_dx = vec![0i32; w * h];
    let mut best_dy = vec![0i32; w * h];

    let cap = ((2 * half + 1) * (2 * half + 1)) as usize;
    let mut offs: Vec<i32> = Vec::with_capacity(cap);
    let mut vals: Vec<[u8; 3]> = Vec::with_capacity(cap);

    for _ in 0..iterations {
        for y in 0..h {
            for x in 0..w {
                let idx = y * w + x;
                if !lvl.hole[idx] {
                    continue;
                }
                // 目标 patch 中「已知」（已填充或本就在洞外）的像素
                offs.clear();
                vals.clear();
                for dy in -half..=half {
                    for dx in -half..=half {
                        let px = x as i32 + dx;
                        let py = y as i32 + dy;
                        if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
                            continue;
                        }
                        let pi = py as usize * w + px as usize;
                        if !known[pi] {
                            continue;
                        }
                        offs.push(dy * w as i32 + dx);
                        let p = pi * 4;
                        vals.push([lvl.img[p], lvl.img[p + 1], lvl.img[p + 2]]);
                    }
                }
                if offs.is_empty() {
                    continue; // 上下文还不够，等下一轮
                }

                // 候选：邻居传播 + 随机采样
                let mut top = [Cand { ssd: u64::MAX, base: 0 }; 3];
                let mut topn = 0usize;

                let consider = |base: usize, top: &mut [Cand; 3], topn: &mut usize| {
                    if !valid[base] {
                        return;
                    }
                    let ssd = patch_ssd(&lvl.img, base, &offs, &vals);
                    let cand = Cand { ssd, base };
                    if *topn < 3 {
                        top[*topn] = cand;
                        *topn += 1;
                    } else if ssd < top[2].ssd {
                        top[2] = cand;
                    } else {
                        return;
                    }
                    // 插入排序（最多 3 个）
                    let mut i = *topn - 1;
                    while i > 0 && top[i].ssd < top[i - 1].ssd {
                        top.swap(i, i - 1);
                        i -= 1;
                    }
                };

                // 左邻居传播
                if x > 0 {
                    let ni = idx - 1;
                    if lvl.hole[ni] {
                        let nx = x as i32 + best_dx[ni] - 1;
                        let ny = y as i32 + best_dy[ni];
                        if nx >= 0 && ny >= 0 && nx < w as i32 && ny < h as i32 {
                            consider(ny as usize * w + nx as usize, &mut top, &mut topn);
                        }
                    }
                }
                // 上邻居传播
                if y > 0 {
                    let ni = idx - w;
                    if lvl.hole[ni] {
                        let nx = x as i32 + best_dx[ni];
                        let ny = y as i32 + best_dy[ni] - 1;
                        if nx >= 0 && ny >= 0 && nx < w as i32 && ny < h as i32 {
                            consider(ny as usize * w + nx as usize, &mut top, &mut topn);
                        }
                    }
                }
                // 随机候选
                for _ in 0..RANDOM_CANDIDATES {
                    let base = rng.below(w * h);
                    consider(base, &mut top, &mut topn);
                }

                if topn == 0 {
                    continue;
                }

                // 投票：取前 3 个候选中心像素的逐通道中位数
                let mut rgb = [0u8; 3];
                for c in 0..3 {
                    let mut v = [0u8; 3];
                    for k in 0..topn {
                        v[k] = lvl.img[top[k].base * 4 + c];
                    }
                    v[..topn].sort_unstable();
                    rgb[c] = v[topn / 2];
                }
                let alpha = lvl.img[top[0].base * 4 + 3];
                lvl.img[idx * 4] = rgb[0];
                lvl.img[idx * 4 + 1] = rgb[1];
                lvl.img[idx * 4 + 2] = rgb[2];
                lvl.img[idx * 4 + 3] = alpha;

                let src_x = (top[0].base % w) as i32;
                let src_y = (top[0].base / w) as i32;
                best_dx[idx] = src_x - x as i32;
                best_dy[idx] = src_y - y as i32;
                known[idx] = true;
            }
        }
    }
}

/// 把最细层的洞内容写回输出图。
///
/// 最细层的坐标就是**整图坐标**（`Level::from_image` 按整图尺寸建），
/// 这里不能再加 `region.x/y`，否则会整体偏移一个选区原点。
fn write_back(out: &mut RgbaImage, lvl: &Level) {
    for y in 0..lvl.h {
        for x in 0..lvl.w {
            let idx = y * lvl.w + x;
            if !lvl.hole[idx] {
                continue;
            }
            let p = idx * 4;
            out.set_pixel(
                x as u32,
                y as u32,
                Rgba {
                    r: lvl.img[p],
                    g: lvl.img[p + 1],
                    b: lvl.img[p + 2],
                    a: lvl.img[p + 3],
                },
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 确定性随机数
// ---------------------------------------------------------------------------------------------

/// 确定性 xorshift64* 发生器（固定种子，不依赖系统时间）。
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }

    #[inline]
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    #[inline]
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % (n as u64)) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// 造一张带渐变 + 噪声的图（确定性）。
    fn noisy_gradient(w: u32, h: u32) -> RgbaImage {
        let mut img = RgbaImage::new(w, h);
        let mut s = 0x1234_5678u32;
        for y in 0..h {
            for x in 0..w {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                let n = (s % 24) as i32 - 12;
                let r = ((x * 255) / w.max(1)) as i32 + n;
                let g = ((y * 255) / h.max(1)) as i32 + n;
                let b = (((x + y) * 255) / (w + h).max(1)) as i32 + n;
                img.set_pixel(
                    x,
                    y,
                    Rgba::rgb(r.clamp(0, 255) as u8, g.clamp(0, 255) as u8, b.clamp(0, 255) as u8),
                );
            }
        }
        img
    }

    #[test]
    fn outside_region_untouched() {
        let img = noisy_gradient(120, 90);
        let region = Rect::new(30, 20, 40, 30);
        let out = inpaint_region(&img, region);
        assert_eq!((out.w, out.h), (img.w, img.h));
        for y in 0..img.h {
            for x in 0..img.w {
                if region.contains(crate::geometry::Point::new(x as i32, y as i32)) {
                    continue;
                }
                assert_eq!(out.pixel(x, y), img.pixel(x, y), "({x},{y}) 不该被修改");
            }
        }
    }

    #[test]
    fn deterministic() {
        let img = noisy_gradient(120, 90);
        let region = Rect::new(30, 20, 40, 30);
        let a = inpaint_region(&img, region);
        let b = inpaint_region(&img, region);
        assert_eq!(a.data, b.data);
    }

    #[test]
    fn degenerate_inputs_do_not_panic() {
        let img = noisy_gradient(60, 40);
        // 越界
        let a = inpaint_region(&img, Rect::new(1000, 1000, 20, 20));
        assert_eq!((a.w, a.h), (img.w, img.h));
        // 空矩形
        let b = inpaint_region(&img, Rect::new(10, 10, 0, 0));
        assert_eq!((b.w, b.h), (img.w, img.h));
        // 负尺寸
        let c = inpaint_region(&img, Rect::new(30, 30, -20, -20));
        assert_eq!((c.w, c.h), (img.w, img.h));
        // 覆盖整图
        let d = inpaint_region(&img, Rect::new(0, 0, 60, 40));
        assert_eq!((d.w, d.h), (img.w, img.h));
        // 0 尺寸图
        let e = inpaint_region(&RgbaImage::new(0, 0), Rect::new(0, 0, 10, 10));
        assert_eq!((e.w, e.h), (0, 0));
        // 单像素图
        let f = inpaint_region(&RgbaImage::new(1, 1), Rect::new(0, 0, 1, 1));
        assert_eq!((f.w, f.h), (1, 1));
    }

    #[test]
    fn hole_is_filled() {
        // 纯色背景 + 一个明显异色方块
        let mut img = RgbaImage::new(80, 80);
        for y in 0..80 {
            for x in 0..80 {
                img.set_pixel(x, y, Rgba::rgb(40, 90, 200));
            }
        }
        let block = Rect::new(20, 25, 30, 30);
        for y in 25..55 {
            for x in 20..50 {
                img.set_pixel(x, y, Rgba::rgb(240, 10, 10));
            }
        }
        let out = inpaint_region(&img, block);
        for y in 25..55 {
            for x in 20..50 {
                let p = out.pixel(x, y);
                let bg = Rgba::rgb(40, 90, 200);
                assert!(
                    (p.r as i32 - bg.r as i32).abs() <= 40
                        && (p.g as i32 - bg.g as i32).abs() <= 40
                        && (p.b as i32 - bg.b as i32).abs() <= 40,
                    "({x},{y}) 填充后仍偏离背景太多: {p:?}"
                );
                assert!(
                    p != Rgba::rgb(240, 10, 10),
                    "({x},{y}) 仍然是原来的异色方块"
                );
            }
        }
    }

    #[test]
    fn perf_large_region_is_fast() {
        let img = noisy_gradient(1920, 1080);
        let region = Rect::new(600, 400, 300, 200);
        let t = Instant::now();
        let out = inpaint_region(&img, region);
        let dt = t.elapsed();
        assert_eq!((out.w, out.h), (1920, 1080));
        assert!(
            dt.as_secs_f64() < 3.0,
            "1920x1080 填 300x200 区域耗时 {:?}，超过 3 秒",
            dt
        );
    }
}
