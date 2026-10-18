# 打包 coderender 的 Windows 安装包（.msi）
#
# 用法（在仓库根目录）：
#   powershell -ExecutionPolicy Bypass -File packaging/build-msi.ps1
#
# 做三件事：
#   1. 确认 WiX 工具链（没有就用 dotnet 装 WiX 4）
#   2. cargo build --release --features msi（build.rs 会生成 WiX 源码并调用 WiX）
#   3. 打印产物路径：target/msi/coderender-<版本>-<架构>.msi
#
# 只想生成 WiX 源码检查：$env:CODERENDER_MSI_WXS_ONLY = "1" 后再跑本脚本

[CmdletBinding()]
param(
    # 目标架构：x64 / arm64 / x86
    [string]$Arch = "x64",
    # 跳过工具链检查（比如你已经有 WiX 3）
    [switch]$NoToolCheck
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
Push-Location $repo
try {
    if (-not $NoToolCheck) {
        $wix = Get-Command wix -ErrorAction SilentlyContinue
        if ($null -eq $wix) {
            $dotnet = Get-Command dotnet -ErrorAction SilentlyContinue
            if ($null -eq $dotnet) {
                throw "找不到 dotnet：请先安装 .NET SDK（https://aka.ms/dotnet-download）"
            }
            Write-Host "未找到 wix，正在安装 WiX（dotnet tool install --global wix）..."
            dotnet tool install --global wix
            if ($LASTEXITCODE -ne 0) {
                throw "WiX 安装失败，请手动执行：dotnet tool install --global wix"
            }
        }
        else {
            Write-Host "使用已安装的 WiX：$($wix.Source)"
        }

        # WiX 5+ 需要接受 OSMF EULA；没接受时会报 WIX7015。
        if ($env:CODERENDER_WIX_ACCEPT_EULA -eq "1") {
            Write-Host "按 CODERENDER_WIX_ACCEPT_EULA=1 自动带上 --acceptEula"
        }
        else {
            Write-Host "提示：WiX 5+ 需要接受 OSMF EULA。若报错 WIX7015，请运行一次 `wix eula accept wix7`，"
            Write-Host "      或设置 `$env:CODERENDER_WIX_ACCEPT_EULA = '1'` 后重试。"
        }
    }

    $target = if ($Arch -eq "x64") { "x86_64-pc-windows-msvc" } elseif ($Arch -eq "arm64") { "aarch64-pc-windows-msvc" } else { "i686-pc-windows-msvc" }
    Write-Host "构建目标：$target"
    cargo build --release --target $target --features msi
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build 失败（见上面的错误输出）"
    }

    $msiDir = Join-Path $repo "target\$target\release\msi"
    $msi = Get-ChildItem -Path $msiDir -Filter *.msi -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1
    if ($null -eq $msi) {
        # 没指定 --target 时 build.rs 用默认 profile 目录
        $msi = Get-ChildItem -Path (Join-Path $repo "target\release\msi") -Filter *.msi -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending |
            Select-Object -First 1
    }
    if ($null -eq $msi) {
        throw "没找到生成的 .msi（$msiDir）"
    }

    Write-Host ""
    Write-Host "完成：$($msi.FullName)（$([math]::Round($msi.Length / 1MB, 1)) MB）"
    Write-Host ""
    Write-Host "安装："
    Write-Host "  msiexec /i `"$($msi.FullName)`""
    Write-Host "静默安装："
    Write-Host "  msiexec /i `"$($msi.FullName)`" /qn /norestart"
    Write-Host "卸载："
    Write-Host "  msiexec /x `"$($msi.FullName)`" /qn /norestart"
}
finally {
    Pop-Location
}
