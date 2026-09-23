# Meng meter

Windows 轻量悬浮球：贴边收成内存柱条，悬停展开圆环占用与整机上下行网速。

## 运行

需要 Node.js、Rust（rustup）和 Visual Studio C++ 生成工具。

```powershell
npm install
npm run tauri dev
```

## 安装包

```powershell
npm install
npm run build
```

安装程序在 `src-tauri/target/release/bundle/nsis/`。安装后会在桌面创建快捷方式。第一次打开时选择是否开机自启、启用哪些功能；之后从桌面快捷方式启动即可，关掉后再点快捷方式也能回来。

## 用法

- 拖到屏幕左右边缘松手：吸附并收成柱条，柱高表示内存占用
- 鼠标悬停柱条：展开胶囊，显示占用百分比和总上下行网速
- 离开约 0.4 秒后重新收起
- 颜色：低于 50% 绿，50–80% 橙，80% 及以上红
- 托盘图标：设置 / 显示 / 退出
