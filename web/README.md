# 已拆分

用户端与管理端已拆成两个独立前端项目：

- `../web-admin`：管理端，构建产物嵌入 `tanzaku-board` 进程
- `../web-user`：用户端主题包（含 `komari-theme.json` + `dist/`），由 board 用户端口按 Komari 方式加载

本目录保留仅为兼容旧脚本，请勿再在此开发。
