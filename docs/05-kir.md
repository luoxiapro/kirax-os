# 阶段五：.kir 应用生态

## .kir 二进制格式

```
偏移   大小   字段
0      4      魔数 "SKIR" (0x52494B53)
4      4      头部版本 (u32 LE)
8      4      代码段偏移 (u32 LE)
12     4      代码段大小 (u32 LE)
16     4      图标数据偏移 (u32 LE)
20     4      图标大小 (u32 LE)
24     4      manifest 偏移 (u32 LE)
28     4      manifest 大小 (u32 LE)
32     ...    代码段（ARMv7 Thumb 机器码）
...            图标（RGBA 位图）
...            manifest（JSON 文本）
```

加载器在 kernel 中 `crates/kirax-core/src/kir.rs` 解析头、校验魔数、提取代码段。

## 用户态 mini libc

- 系统调用接口：`svc #0`，r7=号，r0-r6=参数。
- 提供：`draw_line`、`fill_rect`、`blit`、`poll_input`、`open`/`read`/`write`/`close`。

## kirpm 包管理器

- 仓库：`/apps/repo/` 下的 .kir 文件 + 索引。
- `kirpm install <name>`：下载到 `/apps/`，更新开始菜单。
- `kirpm list` / `kirpm remove <name>`。

## 首批应用

| 应用 | 说明 |
|---|---|
| Hello | 最小示例应用（`apps/hello/manifest.json` 模板，内容 "Hello, KiraX OS!"） |
| About KiraX OS | 信息窗口：版本、架构、Glass 版本 |
| Calculator | 简易计算器 |
| Clock | 实时时钟 |
| Editor | 文本编辑器 |
| Terminal | 串口/pty 终端 |

## 常见坑

| 症状 | 原因 |
|---|---|
| .kir 加载报魔数错 | 文件不是 SKIR，或偏移写错 |
| 应用跑飞 | 代码段没放对地址，或系统调用号错 |
