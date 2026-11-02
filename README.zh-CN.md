# MarkdownNote

个人 Markdown 笔记与知识管理工具，Windows 桌面版本 0.3.0。

## 功能

- Markdown 编辑、分屏预览、自动保存和关闭保护。
- 分类、标签、标题与正文检索及搜索命中高亮。
- 双向链接与反向引用、文本导入和单篇 Markdown 导出。
- 深浅主题、右键管理标签、可调整侧栏、专注与全屏。
- 笔记拖入分类、标签拖入笔记、笔记拖入正文创建双向链接。
- 网页链接通过系统默认浏览器打开。

## 开发

需要 Node.js 20、Rust 1.89.0、MSVC 和 Windows SDK。桌面运行依赖 WebView2 Runtime。

```powershell
npm ci
$env:CARGO_BUILD_JOBS = '1'
$env:MARKDOWNNOTE_PROJECT_DATA_DIR = Join-Path (Get-Location) '.local-data'
$env:MARKDOWNNOTE_PROJECT_LOG_DIR = Join-Path (Get-Location) '.local-logs'
npm run tauri:dev
```

```powershell
npm run test:unit
npm run check:design-tokens
npm run build
cargo build --locked --manifest-path src-tauri/Cargo.toml --bin markdownnote --features tauri/custom-protocol -j 1
```

原生 Windows 测试使用 `scripts/run-rust-tests.ps1 -Filter course_`，串行执行。浏览器交互检查在 `tests/course/`，使用模拟后端，不能替代原生桌面检查。

## 数据与范围

完整程序双击 MarkdownNote.exe，同目录需有 `.markdownnote-app` 标记，内容为 `com.course.markdownnote`。
新库使用 `运行数据/markdownnote.sqlite`。既有笔记库按结构识别，保持原文件原地使用；不会为更名自动迁移、合并或删除数据库。
关闭程序后可备份完整运行数据目录。当前没有云同步、插件系统、移动端、多人协作或 PDF 导出。

已删除不再使用的旧界面、独立 Agent 测试程序、模型/翻译开发脚本、发布工作流、截图与历史交付文档。当前笔记功能仍依赖的一部分共用后端实现继续保留；保留代码不表示相关旧功能已启用。

## 许可

许可见 LICENSE，原作者署名见 NOTICE；第三方组件说明见 `文档/开源许可/第三方许可.md`。本项目保留适用的 AGPL-3.0 义务。
