# 开发期静态质量报告

普通 PR 的静态质量发现作为非阻断报告保留，不表示问题已修复。
Development Quality Reports 在 PR、main、定期和手动运行，保留原始输出、
退出码、结构化分类及 check summary；artifact 缺失或上传失败仍报错。
没有新增 Issue 写权限，报告按 workflow/run/check 区分，不生成重复修复任务。

Tea 对 root 与 detached Tauri 做格式及 Clippy 报告，前端 ESLint 独立报告。业务开发仍暂停。
分类器验证实际输出与退出码一致；未知诊断、语法/配置错误、空扫描、
缺失或过时报告和解析失败仍失败，超时也保留已有输出。
Windows main/dispatch 产物构建仍设置 QUALITY_STRICT=true，静态发现继续阻断候选包；普通 PR 为 advisory。

功能测试、类型检查、编译、来源及安全契约不放宽。
正式发布 workflow、签名、部署及本地严格入口不变；开发报告不授予发布资格。
代码设计标准及历史问题仍需遵守和后续修复。
