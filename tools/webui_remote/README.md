# WebUI 外部访问桥接

DevMask 的 WebUI 是给 KernelSU / Magisk 管理器 WebView 用的：页面本身是纯静态的
module/webroot，但所有能力（读配置、写配置、枚举应用、模块信息）都依赖宿主注入的
window.ksu 桥接。把 webroot 丢到任意静态服务器上只能看到空壳页面。

本工具在电脑上起一个 HTTP 服务：

1. 静态托管 module/webroot（Vite 构建产物）；
2. 往 index.html 注入 shim.js，在应用加载前定义 window.ksu（exec / io / pm /
   moduleInfo / toast / 图标协议）；
3. 通过 adb 把这些调用转发到已连接的 Android 设备。

于是可以在电脑或局域网其它设备的浏览器里，直接使用模块的真实 WebUI。

## 快速开始

    # 1. 构建 WebUI（产物输出到 module/webroot）
    cd webui && npm ci && npm run build && cd ..

    # 2. 手机连上电脑，确认 adb 可见
    adb devices

    # 3. 启动桥接服务（默认监听 0.0.0.0:8080）
    node tools/webui_remote/server.mjs --port 8080

启动后终端会打印访问地址：

- 本机：http://127.0.0.1:8080
- 局域网：http://<电脑的局域网 IP>:8080

浏览器打开即可，页面里的配置、模板、应用列表都来自真机。

## 只看界面（不连设备）

Vite 开发模式自带 mock 数据，不需要 adb：

    cd webui && npm run dev -- --host

打开终端打印的地址（默认 3000 端口），可以预览 UI 与交互，但读写的是前端假数据。

## 参数

| 参数 | 说明 | 默认值 |
| --- | --- | --- |
| --host | 监听地址 | 0.0.0.0 |
| --port | 监听端口 | 8080 |
| --root | 静态目录 | module/webroot |
| --serial | 指定 adb 设备序列号 | 唯一在线设备 |
| --adb | adb 可执行文件路径 | adb |
| --timeout | 单条 shell 命令超时（秒） | 60 |

## 无线 adb

不想插线时，可以先让 adb 走网络，桥接服务仍然跑在电脑上：

    adb connect <手机 IP>:5555

（Android 11+ 的「无线调试」需要先按系统提示配对，再用 adb connect 连接。）

## 能力与限制

可用：

- 读取、保存 /data/adb/devmask/config/config.toml（模板、应用配置）；
- 应用列表，包括多用户分身（包名带 @userId 后缀）；
- 模块信息（版本、作者、描述、启用状态，来自 module.prop）；
- toast 提示、shell 命令（在线模板抓取、getprop 等）。

限制：

- 应用名和图标由管理器的 PackageManager 提供，adb shell 拿不到：列表显示包名，
  图标回退为占位图。页面里的 ksu://icon/<包名> 会被改写成桥接地址并返回 404，
  由 WebUI 自行回退。
- exec 的权限就是 adb shell 的权限。有些设备 adb shell 默认是 root（本机测试机就是
  这种情况），有些需要先执行 adb root 或在命令里套 su，否则读不到 /data/adb。
- 桥接服务必须能访问到设备：服务跑在电脑上，不是手机上的常驻服务；手机断开 adb
  后页面还能打开，但读写配置和应用列表会失败。

## 安全提示

桥接接口等价于把设备的 shell 暴露给能打开这个页面的人。默认监听 0.0.0.0 是为了
局域网访问；只在本机使用请加 --host 127.0.0.1，不要在不可信网络里直接暴露端口。

## 文件说明

| 文件 | 作用 |
| --- | --- |
| server.mjs | HTTP 服务、静态托管、KernelSU 桥接接口、adb 后端 |
| shim.js | 浏览器侧的 window.ksu 垫片（同步 XHR + fetch） |
