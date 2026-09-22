<#
.SYNOPSIS
    在本机（沙箱受限环境）激活构建所需的环境变量。

.DESCRIPTION
    本机是一套**完全自包含**的便携 Tauri 工具链（Rust + Node + MinGW + Tauri CLI），
    全部位于 D:\tools\Tauri，不写注册表、不改系统 PATH。使用方式二选一：

      1) 推荐：直接双击 D:\tools\Tauri\TauriShell.cmd 打开一个已激活的 PowerShell；
      2) 在现有会话里激活（必须带 -ExecutionPolicy Bypass，本机执行策略为 Restricted）：

           powershell -ExecutionPolicy Bypass -NoExit -File D:\tools\Tauri\env.ps1

    本脚本是第 2 种的**薄封装**，额外做两件与项目相关的事：
      - 把 target\debug 加进 PATH：Tauri 的 WebView2Loader.dll 与 exe 同级，
        而 `cargo test` 生成的测试可执行文件在 target\debug\deps 下，
        不加这条路径测试程序会因为找不到 DLL 而启动失败（STATUS_ENTRYPOINT_NOT_FOUND）。
      - 检查 crates 转发服务（本机无法 HTTPS，cargo 依赖它走明文镜像）。

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\activate-env.ps1
#>
[CmdletBinding()]
param(
    [string]$TauriRoot = 'D:\tools\Tauri'
)

$envScript = Join-Path $TauriRoot 'env.ps1'
if (-not (Test-Path $envScript)) {
    throw "找不到工具链激活脚本：$envScript（用 -TauriRoot 指定正确路径）"
}

# 复用官方激活脚本（它负责 RUSTUP_HOME / CARGO_HOME / PATH / 镜像 / crates 转发服务）
. $envScript

# 项目相关补丁：测试可执行文件在 deps 下，需要能加载 target\debug 里的 WebView2Loader.dll
$projectRoot = Split-Path -Parent $PSScriptRoot
$debugDir = Join-Path $projectRoot 'src-tauri\target\debug'
if ((Test-Path $debugDir) -and ($env:PATH -notlike "*$debugDir*")) {
    $env:PATH = "$debugDir;$env:PATH"
    Write-Host "[env] 已加入 $debugDir（供测试程序加载 WebView2Loader.dll）" -ForegroundColor Green
}

Write-Host "[env] 项目根目录：$projectRoot" -ForegroundColor Cyan
Write-Host '[env] 可用命令：pnpm run build | pnpm tauri dev | pnpm tauri build --no-bundle | cargo test --lib' -ForegroundColor Cyan
