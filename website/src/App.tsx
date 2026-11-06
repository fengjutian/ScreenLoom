import { BUILD_STEPS, CLI_ROWS, FEATURES, LIMITS, NAV_ITEMS, USE_CASES } from './data/features.ts'

function Nav() {
  return <nav className="nav" aria-label="主导航"><div className="container nav-inner"><a className="nav-brand" href="#top" aria-label="screenloom 首页"><img src="/icon.png" alt="" />screenloom</a><div className="nav-links">{NAV_ITEMS.map((item) => <a key={item.id} href={`#${item.id}`}>{item.label}</a>)}<a className="nav-cta" href="#build">开始使用</a></div></div></nav>
}

function Hero() {
  return <header className="hero" id="top"><div className="container hero-inner"><div className="hero-copy"><div className="hero-badges"><span className="badge"><i className="badge-dot" />Windows 原生体验</span><span className="badge">本地优先</span><span className="badge">开源 · Rust</span></div><p className="hero-kicker">截图、识别、标注、录屏，一次完成</p><h1>把屏幕内容，<span className="accent">立刻变成可用素材</span></h1><p className="hero-lead">screenloom 是一款面向 Windows 的本地截图与录屏工具。选区后即可标注、提取文字、生成长截图或保存视频，常用处理无需上传云端。</p><div className="hero-actions"><a className="btn btn-primary" href="#build">从源码构建 <span aria-hidden="true">→</span></a><a className="btn btn-ghost" href="#gallery">查看真实界面</a></div><p className="hero-note">默认热键 <code>Alt+A</code> 截图 · <code>Alt+R</code> 录屏</p></div><div className="hero-visual" aria-label="screenloom 选区工具栏预览"><div className="visual-glow" /><div className="app-preview"><div className="shot-bar"><i /><i /><i /><span>screenloom · 选区工具栏</span></div><img src="/img/toolbar.png" alt="screenloom 选区后的浮动工具栏" /></div><div className="privacy-chip">图像默认留在本机</div></div></div></header>
}

function ProofBar() {
  return <section className="proof-bar" aria-label="产品特点"><div className="container proof-grid"><div><strong>本地处理</strong><span>截图、OCR 与标注不必上传</span></div><div><strong>原生能力</strong><span>调用 Windows 捕获与识别接口</span></div><div><strong>工作流完整</strong><span>从捕获到保存一次完成</span></div></div></section>
}

function Features() {
  return <section id="features"><div className="container"><div className="section-head split-head"><div><span className="eyebrow">核心能力</span><h2>少切换几个应用，多完成一步工作</h2></div><p>从抓取屏幕到拿到最终文件，常见操作都在同一条本地工作流中完成。只有主动使用 AI 功能时才需要配置自己的 API Key。</p></div><div className="feature-grid">{FEATURES.map((feature, index) => <article className="feature-card" key={feature.title}><span className="feature-index">0{index + 1}</span>{feature.tag && <span className={`tag tag-${feature.tagTone ?? 'local'}`}>{feature.tag}</span>}<h3>{feature.title}</h3><p>{feature.desc}</p></article>)}</div></div></section>
}

function UseCases() {
  return <section className="surface-section" id="use-cases"><div className="container"><div className="section-head"><span className="eyebrow">适合这些时刻</span><h2>从“看到了”到“拿去用”</h2><p>screenloom 不只负责按下快门，也把截图之后最常见的几步接了起来。</p></div><div className="use-case-grid">{USE_CASES.map((item) => <article className="use-case" key={item.title}><span className="use-case-label">{item.label}</span><h3>{item.title}</h3><p>{item.desc}</p><div className="use-case-flow">{item.flow}</div></article>)}</div></div></section>
}

function Gallery() {
  return <section id="gallery"><div className="container"><div className="section-head split-head"><div><span className="eyebrow">真实界面</span><h2>看到什么，就得到什么</h2></div><p>以下均为程序实际运行截图。界面保持克制，让内容和下一步操作始终处在视觉中心。</p></div><div className="gallery"><figure className="shot shot-featured"><div className="shot-bar"><i /><i /><i /><span>选区工具栏</span></div><img src="/img/toolbar.png" alt="screenloom 选区浮动工具栏" /><figcaption className="shot-cap"><strong>截完就能继续处理</strong>标注、撤销、OCR、长截图、复制与保存，都在选区旁边完成。</figcaption></figure><figure className="shot"><div className="shot-bar"><i /><i /><i /><span>OCR 结果</span></div><img src="/img/ocr.png" alt="本地 OCR 文字识别结果" /><figcaption className="shot-cap"><strong>本地提取文字</strong>识别结果进入面板并写入剪贴板，无需联网或 API Key。</figcaption></figure><figure className="shot"><div className="shot-bar"><i /><i /><i /><span>设置</span></div><img className="shot-tall" src="/img/settings.png" alt="screenloom 设置窗口" /><figcaption className="shot-cap"><strong>设置清楚，也保持简单</strong>热键、长截图与 AI 配置集中管理，修改后即时生效。</figcaption></figure></div></div></section>
}

function Cli() {
  return <section className="surface-section" id="cli"><div className="container"><div className="section-head"><span className="eyebrow">自动化</span><h2>不只可以点，也可以调用</h2><p>常驻模式负责热键和托盘；命令行参数则适合脚本、快捷方式与自动化流程。</p></div><div className="cmd-table">{CLI_ROWS.map((row) => <div className="cmd-row" key={row.flag}><code>{row.flag}</code><span>{row.desc}</span></div>)}</div></div></section>
}

function Build() {
  return <section id="build"><div className="container build-wrap"><div className="build-intro"><span className="eyebrow">开始使用</span><h2>三步在 Windows 上跑起来</h2><p>项目目前以源码构建方式提供。准备好 Rust 工具链后，即可编译 release 版本并常驻运行。</p><div className="requirements"><span>Windows 10 1903+</span><span>Rust 1.75+</span><span>x64</span></div></div><div className="steps">{BUILD_STEPS.map((step, index) => <div className="step" key={step.title}><h3><span className="step-num">{index + 1}</span>{step.title}</h3><p>{step.desc}</p><pre><code>{step.code}</code></pre></div>)}</div></div></section>
}

function Limits() {
  return <section className="surface-section" id="limits"><div className="container"><div className="section-head"><span className="eyebrow">当前边界</span><h2>适合什么，也说清楚暂时不适合什么</h2><p>这是仍在演进的开源项目。下面这些限制会直接影响使用选择。</p></div><div className="limits">{LIMITS.map((limit) => <div className="limit" key={limit.title}><h3>{limit.title}</h3><p>{limit.desc}</p></div>)}</div></div></section>
}

function Footer() {
  return <footer className="footer"><div className="container footer-inner"><div><a className="footer-brand" href="#top">screenloom</a><p>用 Rust 构建的 Windows 本地截图与录屏工具。</p></div><a href="#top">回到顶部 ↑</a></div></footer>
}

export default function App() {
  return <><Nav /><main><Hero /><ProofBar /><Features /><UseCases /><Gallery /><Cli /><Build /><Limits /></main><Footer /></>
}
