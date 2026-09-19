<#
.SYNOPSIS
    把上游 dsh-pet 的完整动画素材导入本应用的素材目录。

.DESCRIPTION
    应用运行期只从「应用数据目录 / webm」读取动画（用户可直接往里放自己的透明动画）。
    本脚本负责把上游仓库里的 106 条 VP9-alpha 透明动画一次性导进去。

    上游仓库**只读**：本脚本只做拷贝，从不修改上游任何文件。

.PARAMETER SourceRepo
    上游仓库根目录，默认为 D:\programs\github\whale-pet。

.PARAMETER All
    连同 assets 下的表情包 / 通知图标 / 字体一起导入（M1 以后才会用到，默认不导）。

.EXAMPLE
    pwsh -File scripts\import-animations.ps1
    pwsh -File scripts\import-animations.ps1 -SourceRepo 'E:\github\whale-pet' -All
#>
[CmdletBinding()]
param(
    [string]$SourceRepo = 'D:\programs\github\whale-pet',
    [switch]$All
)

$ErrorActionPreference = 'Stop'

# 应用数据目录：与 Rust 侧 AppState.app_data_dir 一致（Tauri 的 identifier = com.whalepet.desktop）
$AppDataDir = Join-Path $env:APPDATA 'com.whalepet.desktop'
$TargetWebm = Join-Path $AppDataDir 'webm'

$SourceWebm = Join-Path $SourceRepo 'dsh-pet\assets\webm'
if (-not (Test-Path $SourceWebm)) {
    throw "在上游仓库里找不到动画目录：$SourceWebm（用 -SourceRepo 指定正确路径）"
}

# 只允许新增/覆盖 webm 素材目录，绝不删除用户已有文件
New-Item -ItemType Directory -Force -Path $TargetWebm | Out-Null

Write-Host "[导入] 上游素材: $SourceWebm" -ForegroundColor Cyan
Write-Host "[导入] 目标目录: $TargetWebm" -ForegroundColor Cyan

$files = Get-ChildItem -Path $SourceWebm -File -Filter '*.webm'
if ($files.Count -eq 0) {
    throw "$SourceWebm 下没有 .webm 文件（上游仓库是否完整？）"
}

$copied = 0
$skipped = 0
foreach ($file in $files) {
    $dest = Join-Path $TargetWebm $file.Name
    # 已存在且大小一致 => 跳过（避免重复导入时无谓地重写上百 MB）
    if ((Test-Path $dest) -and ((Get-Item $dest).Length -eq $file.Length)) {
        $skipped++
        continue
    }
    Copy-Item -LiteralPath $file.FullName -Destination $dest -Force
    $copied++
}

$totalMb = [math]::Round((Get-ChildItem $TargetWebm -File | Measure-Object Length -Sum).Sum / 1MB, 1)
Write-Host "[导入] 完成：新拷 $copied 个，跳过 $skipped 个；素材目录现有 $($files.Count) 条 / $totalMb MB" -ForegroundColor Green

if ($All) {
    # 表情包 / 通知图标 / 气泡字体：气泡与碎碎念功能会用到
    #
    # 写脚本踩到的两个坑，记在这里避免重复：
    #   1) 哈希字面量的键名不能用 From —— 它是 PowerShell 保留字，会报 MissingArrayIndexExpression；
    #   2) 注释里不要写反引号（它在本语言里是转义字符，即使在 # 注释中也会让解析器吃掉后续字符）。
    $pairs = @(
        @{ Src = 'dsh-pet\assets\memes'; Dst = 'memes' },
        @{ Src = 'dsh-pet\assets\pic'; Dst = 'pic' },
        @{ Src = 'dsh-pet\assets\fonts'; Dst = 'fonts' }
    )
    foreach ($pair in $pairs) {
        $source = Join-Path $SourceRepo $pair.Src
        if (-not (Test-Path $source)) { continue }
        $dest = Join-Path $AppDataDir $pair.Dst
        New-Item -ItemType Directory -Force -Path $dest | Out-Null
        Copy-Item -Path (Join-Path $source '*') -Destination $dest -Recurse -Force
        Write-Host "[导入] 附加素材 -> $dest" -ForegroundColor Green
    }
}

Write-Host ''
Write-Host '提示：动画文件名（含扩展名）要写进 config.jsonc 的 pets[].idle / pets[].click；' -ForegroundColor Yellow
Write-Host '      留空则会自动挑选目录里排序最靠前的那个。改完配置重启应用生效。' -ForegroundColor Yellow
