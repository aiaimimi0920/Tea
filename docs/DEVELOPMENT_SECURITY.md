# 开发期安全检查

本轮只配置安全检查，不恢复 Tea 暂停的业务开发。
OSV 明确扫描根 Cargo.lock、桌面 Cargo.lock 和桌面 package-lock.json；
CodeQL 扫描 Rust、JavaScript/TypeScript 与 Actions，不编译产品。
Dependabot 对应上述依赖和 Actions 按周创建有限数量的更新 PR。

普通功能 PR 和主干的依赖发现保留为报告，工具下载/执行/解析/覆盖缺失及
上传错误仍失败。原有 cargo-audit 与 npm audit 保留原始 JSON 和退出码。
PR check summary 与 artifact 是报告入口；OSV/CodeQL 使用稳定 SARIF 指纹
由 GitHub 去重，不自动创建重复修复任务，也不增加 issue-write 权限。

release-tea-tag.yml 不变，发布审核、签名、部署和业务测试门槛不放宽。
未新增漏洞例外。扫描成功不等于漏洞修复；仓库原生安全开关另行核实。
