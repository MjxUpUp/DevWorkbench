<!-- FORGE:START -->

# Forge 质量协议

本项目使用 Forge 进行质量保障。请遵守以下规则：

1. **修改前先说意图** — 告诉用户你打算改什么、为什么改
2. **编译必须通过** — 每次修改后确认编译通过（auto-compile hook 自动检查）
3. **不弱化断言** — 不删除 t.Fatal、assert! 等断言（assertion-check hook 自动检查）
4. **测试伴随变更** — 新代码有对应测试
5. **提交前确认** — commit 信息描述变更内容和原因
6. **结束前验证** — 会话结束前运行测试确认无破坏

使用 `/forge-pipeline` 运行项目级管道。
使用 `/forge-quality` 查看完整质量协议。

<!-- FORGE:END -->
