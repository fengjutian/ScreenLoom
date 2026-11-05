import {
  ARCH_FLOW,
  BUILD_STEPS,
  CLI_ROWS,
  FEATURES,
  LAYERS,
  LIMITS,
  NAV_ITEMS,
} from './data/features.ts'

/** 顶部导航。 */
function Nav() {
  return (
    <nav className="nav">
      <div className="container nav-inner">
        <a className="nav-brand" href="#top">
          <img src="/icon.png" alt="" />
          screenloom
        </a>
        <div className="nav-links">
          {NAV_ITEMS.map((item) => (
            <a key={item.id} href={`#${item.id}`}>
              {item.label}
            </a>
          ))}
          <a className="nav-cta" href="#build">
            构建
          </a>
        </div>
      </div>
    </nav>
  )
}

/** 首屏。 */
function Hero() {
  return (
    <header className="hero" id="top">
      <div className="container hero-inner">
        <div className="hero-badges">
          <span className="badge">
            <i className="badge-dot" />
            Windows 专用
          </span>
          <span className="badge">Rust 2021</span>
          <span className="badge">仅 5 个依赖 crate</span>
        </div>
        <h1>
          Windows 上的
          <br />
          <span className="accent">纯 Rust 截图录屏工具</span>
        </h1>
        <p className="hero-lead">
          区域截图、标注编辑、滚动长截图、本地 OCR、二维码识别、AI 问图翻译打码，
          以及 H.264 录屏导出——全部在本地完成，不依赖任何外部进程。
        </p>
        <div className="hero-actions">
          <a className="btn btn-primary" href="#build">
            从源码构建
          </a>
          <a className="btn btn-ghost" href="#features">
            看看能做什么
          </a>
        </div>
        <p className="hero-note">
          默认热键 <code>Alt+A</code> 截图 · <code>Alt+R</code> 录屏
        </p>
      </div>
    </header>
  )
}

/** 功能卡片。 */
function Features() {
  return (
    <section id="features">
      <div className="container">
        <div className="section-head">
          <span className="eyebrow">功能</span>
          <h2>把截图这件事做完整</h2>
          <p>
            从抓屏到成片，中间每一步都留在你自己的机器上。标注与 OCR 走系统能力，
            只有问 AI 那三个功能需要自备 API key。
          </p>
        </div>
        <div className="feature-grid">
          {FEATURES.map((f) => (
            <article className="feature-card" key={f.title}>
              {f.tag && (
                <span className={`tag tag-${f.tagTone ?? 'local'}`}>{f.tag}</span>
              )}
              <h3>{f.title}</h3>
              <p>{f.desc}</p>
            </article>
          ))}
        </div>
      </div>
    </section>
  )
}

/** 真实界面截图。 */
function Gallery() {
  return (
    <section id="gallery">
      <div className="container">
        <div className="section-head">
          <span className="eyebrow">界面</span>
          <h2>真实界面，不是效果图</h2>
          <p>下面每一张都是程序真机跑出来的截图，没有一处是设计稿。</p>
        </div>
        <div className="gallery">
          <figure className="shot">
            <div className="shot-bar">
              <i />
              <i />
              <i />
              <span>设置窗口</span>
            </div>
            <img
              className="shot-tall"
              src="/img/settings.png"
              alt="screenloom 设置窗口"
            />
            <figcaption className="shot-cap">
              <strong>设置窗口</strong>
              常规、长截图、AI 三组配置，改完即时写盘生效。
            </figcaption>
          </figure>

          <figure className="shot">
            <div className="shot-bar">
              <i />
              <i />
              <i />
              <span>结果面板</span>
            </div>
            <img src="/img/ocr.png" alt="OCR 提取文字结果面板" />
            <figcaption className="shot-cap">
              <strong>本地 OCR 结果</strong>
              识别结果进面板并写进剪贴板，不需要联网也不需要 key。
            </figcaption>
          </figure>

          <figure className="shot shot-wide">
            <div className="shot-bar">
              <i />
              <i />
              <i />
              <span>选区工具栏</span>
            </div>
            <img src="/img/toolbar.png" alt="screenloom 选区浮动工具栏" />
            <figcaption className="shot-cap">
              <strong>选区浮动工具栏</strong>
              图形标注、调色板、撤销重做，以及问 AI、翻译、AI 打码、提取文字、
              滚动截图、设置、复制、保存图片、置顶在桌面等操作。
            </figcaption>
          </figure>

          <figure className="shot shot-wide">
            <div className="shot-bar">
              <i />
              <i />
              <i />
              <span>长截图提示</span>
            </div>
            <div className="shot-panel">
              <img src="/img/longshot.png" alt="滚动截图提示窗口" />
            </div>
            <figcaption className="shot-cap">
              <strong>滚动长截图</strong>
              你滚页面，程序负责按像素匹配拼接；Enter 继续拼，Esc 结束并落盘。
            </figcaption>
          </figure>
        </div>
      </div>
    </section>
  )
}

/** 命令行参考。 */
function Cli() {
  return (
    <section id="cli">
      <div className="container">
        <div className="section-head">
          <span className="eyebrow">命令行</span>
          <h2>每个能力都有对应的参数</h2>
          <p>
            常驻模式只负责挂热键，其余功能都能脚本化调用，适合接进自动化流程。
          </p>
        </div>
        <div className="cmd-table">
          {CLI_ROWS.map((row) => (
            <div className="cmd-row" key={row.flag}>
              <code>{row.flag}</code>
              <span>{row.desc}</span>
            </div>
          ))}
        </div>
      </div>
    </section>
  )
}

/** 架构分层。 */
function Architecture() {
  return (
    <section id="architecture">
      <div className="container">
        <div className="section-head">
          <span className="eyebrow">架构</span>
          <h2>四层，依赖单向不回头</h2>
          <p>
            分层边界写死在契约里：core 碰不到 Win32，ui 回头依赖 app 就算循环依赖。
          </p>
        </div>
        <div className="layer-list">
          {LAYERS.map((layer) => (
            <div className="layer" key={layer.name}>
              <div className="layer-name">{layer.name}</div>
              <div>
                <div className="layer-desc">{layer.desc}</div>
                <div className="layer-rule">{layer.rule}</div>
              </div>
            </div>
          ))}
        </div>
        <div className="arch-flow">{ARCH_FLOW}</div>      </div>
    </section>
  )
}

/** 构建与运行。 */
function Build() {
  return (
    <section id="build">
      <div className="container">
        <div className="section-head">
          <span className="eyebrow">构建</span>
          <h2>三步跑起来</h2>
          <p>
            仓库是标准 Cargo workspace，克隆后装好 Rust 工具链即可编译，没有额外构建脚本依赖。
          </p>
        </div>
        <div className="steps">
          {BUILD_STEPS.map((step, i) => (
            <div className="step" key={step.title}>
              <h3>
                <span className="step-num">{i + 1}</span>
                {step.title}
              </h3>
              <p>{step.desc}</p>
              <pre>
                <code>{step.code}</code>
              </pre>
            </div>
          ))}
        </div>
      </div>
    </section>
  )
}

/** 已知限制。 */
function Limits() {
  return (
    <section id="limits">
      <div className="container">
        <div className="section-head">
          <span className="eyebrow">边界</span>
          <h2>哪些还没做</h2>
          <p>把限制写在前面，比让你用的时候才发现要好。</p>
        </div>
        <div className="limits">
          {LIMITS.map((limit) => (
            <div className="limit" key={limit.title}>
              <h4>{limit.title}</h4>
              <p>{limit.desc}</p>
            </div>
          ))}
        </div>
      </div>
    </section>
  )
}

/** 页脚。 */
function Footer() {
  return (
    <footer className="footer">
      <div className="container footer-inner">
        <span>screenloom · 纯 Rust 编写的 Windows 截图与录屏工具</span>
        <span style={{ marginLeft: 'auto' }}>
          <a href="#top">回到顶部</a>
        </span>
      </div>
    </footer>
  )
}

export default function App() {
  return (
    <>
      <Nav />
      <Hero />
      <Features />
      <Gallery />
      <Cli />
      <Architecture />
      <Build />
      <Limits />
      <Footer />
    </>
  )
}
