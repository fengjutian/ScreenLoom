# screenloom 官网

screenloom 的介绍站点：React 19 + TypeScript + Vite，纯静态，部署到 Netlify。

## 本地开发

```powershell
cd website
npm install
npm run dev        # http://localhost:5173
```

| 命令 | 说明 |
|---|---|
| `npm run dev` | 开发服务器，带 HMR |
| `npm run build` | 生产构建（先 `tsc -b` 类型检查，再 `vite build`），产物在 `dist/` |
| `npm run preview` | 本地预览生产构建 |
| `npm run lint` | oxlint 静态检查 |

## 目录

```
website/
├─ index.html          页面骨架 + SEO meta
├─ netlify.toml        Netlify 部署配置
├─ public/
│  ├─ icon.png         应用图标（取自 screenloom-app/assets）
│  └─ img/             真实界面截图（从 probes/e2e_out 裁剪）
└─ src/
   ├─ App.tsx          页面各 section
   ├─ index.css        设计令牌与样式（配色取自 theme.dev.json）
   ├─ data/features.ts 站点文案与数据
   └─ main.tsx         入口
```

## 内容维护

站点文案集中在 `src/data/features.ts`：功能卡片、命令行参数表、架构分层、
已知限制都在那里改，不用碰组件。

**图片素材**：`public/img/` 下的截图来自 `probes/e2e_out/`（E2E 真机留证）。
重新生成留证后按同样边界裁剪覆盖即可，注意裁掉桌面上其他窗口，避免带入个人信息。

## 部署到 Netlify

### 方式一：Git 集成（推荐）

1. 在 Netlify 后台 `Add new site` → `Import an existing project`，选本仓库。
2. 填构建参数（`netlify.toml` 已写好，一般会自动识别）：

   | 项 | 值 |
   |---|---|
   | Base directory | `website` |
   | Build command | `npm run build` |
   | Publish directory | `dist` |
   | Node version | `22` |

3. Deploy。后续每次 push 自动部署。

### 方式二：拖拽部署

```powershell
cd website
npm run build
```

把 `website/dist` 整个文件夹拖到 Netlify 后台的 `Deploys` → `Upload a folder`。

### 方式三：Netlify CLI

```powershell
npm i -g netlify-cli
netlify deploy --dir=website/dist --prod
```

## 说明

- 站点是单页应用，`netlify.toml` 里配了 `/* → /index.html`（状态 200），
  直接访问任意路径刷新都不会 404。
- `dist/` 与 `node_modules/` 已在 `.gitignore` 里，不入库。
- 配色沿用应用自己的 `theme.dev.json`，改主题时两边同步。
