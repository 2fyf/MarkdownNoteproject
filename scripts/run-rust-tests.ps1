<#
.SYNOPSIS
  在 Windows 上可复现地运行 Rust 单元测试（含 Common Controls v6 清单处理的测试副本）。

.DESCRIPTION
  背景：直接 `cargo test` 时，测试程序在 Windows 上可能以 0xc0000139
  (STATUS_ENTRYPOINT_NOT_FOUND) 在执行任何测试前退出 —— 原因是测试可执行文件没有
  应用清单，加载的是 comctl32.dll v5，缺少 tauri/wry 用到的 TaskDialogIndirect 等入口。

  本脚本不改动正式构建：它先 `cargo test --no-run` 拿到测试可执行文件，复制到
  `src-tauri/target/course-test-run/`，给副本嵌入一份 Common Controls v6 标准清单
  （mt.exe），再串行执行副本。找不到 mt.exe 时给出明确提示；确实想直接跑也可以加
  -SkipManifest。

.PARAMETER Filter
  测试名过滤器。默认空（跑 --lib 的全部单元测试）。只跑本项目新增的搜索/标签与路径测试：
  -Filter course_

.PARAMETER SkipManifest
  跳过清单嵌入，直接运行原始测试可执行文件（可能复现 0xc0000139）。

.PARAMETER DryRun
  只解析并打印将要执行的命令与产物路径，不编译、不运行。

.EXAMPLE
  powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-rust-tests.ps1 -Filter course_
#>
[CmdletBinding()]
param(
    [string]$Filter = '',
    [switch]$SkipManifest,
    [switch]$DryRun,
    [switch]$Offline
)

$ErrorActionPreference = 'Stop'

$projectRoot = Split-Path -Parent $PSScriptRoot
$manifestPath = Join-Path $projectRoot 'src-tauri\Cargo.toml'
$targetRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $projectRoot 'src-tauri\target' }
$workRoot = Join-Path $targetRoot 'course-test-run'
$manifestFile = Join-Path $workRoot 'common-controls.manifest'
$testLog = Join-Path $workRoot 'cargo-test-no-run.json'

$commonControlsManifest = @'
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="MarkdownNoteCourseTests" version="1.0.0.0" processorArchitecture="*"/>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0"
                        processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>
'@

function Find-MtExe {
    $fromPath = Get-Command mt.exe -ErrorAction SilentlyContinue
    if ($fromPath) { return $fromPath.Source }
    $roots = @()
    if ($env:WindowsSdkDir) { $roots += (Join-Path $env:WindowsSdkDir 'bin') }
    foreach ($prefix in @(${env:ProgramFiles(x86)}, $env:ProgramFiles)) {
        if ($prefix) {
            $candidate = Join-Path $prefix 'Windows Kits\10\bin'
            if (Test-Path -LiteralPath $candidate) { $roots += $candidate }
        }
    }
    foreach ($root in $roots) {
        if (-not (Test-Path -LiteralPath $root)) { continue }
        $hit = Get-ChildItem -LiteralPath $root -Filter 'mt.exe' -Recurse -File -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match '\\x64\\' } |
            Sort-Object -Property FullName -Descending |
            Select-Object -First 1
        if ($hit) { return $hit.FullName }
    }
    return $null
}

try {
    if (-not (Test-Path -LiteralPath $manifestPath)) {
        throw ('找不到 Cargo.toml：{0}' -f $manifestPath)
    }
    $cargo = Get-Command cargo -ErrorAction SilentlyContinue
    if (-not $cargo) {
        if ($DryRun) {
            Write-Host 'cargo: 未找到（DryRun 仍可检查路径）' -ForegroundColor Yellow
            Write-Host ('工程根   : {0}' -f $projectRoot)
            Write-Host ('Cargo.toml: {0}' -f $manifestPath)
            Write-Host ('测试副本 : {0}' -f $workRoot)
            Write-Host '提示：安装 Rust 1.89.0（rust-toolchain.toml）后重试。'
            exit 0
        }
        throw 'PATH 上找不到 cargo；请先安装 Rust 工具链（rust-toolchain.toml 指定 1.89.0）。'
    }

    $mt = $null
    if (-not $SkipManifest) { $mt = Find-MtExe }

    if ($DryRun) {
        Write-Host 'Rust 测试运行计划（DryRun，不编译不运行）' -ForegroundColor Cyan
        Write-Host ('  cargo       : {0}' -f $cargo.Source)
        Write-Host ('  工程根      : {0}' -f $projectRoot)
        Write-Host ('  Cargo.toml  : {0}' -f $manifestPath)
        Write-Host ('  测试副本目录: {0}' -f $workRoot)
        Write-Host ('  清单文件    : {0}' -f $manifestFile)
        Write-Host ('  mt.exe      : {0}' -f $(if ($mt) { $mt } else { '未找到（将提示或需要 -SkipManifest）' }))
        Write-Host ('  过滤器      : {0}' -f $(if ($Filter) { $Filter } else { '(无：跑 --lib 全部)' }))
        Write-Host '  将执行      : cargo test --manifest-path <Cargo.toml> --no-run --message-format=json -j 1 --lib'
        Write-Host '                → 复制测试 exe → mt.exe 嵌入清单 → 串行运行副本'
        exit 0
    }

    if (-not $SkipManifest -and -not $mt) {
        Write-Host '未找到 mt.exe（Windows SDK）。' -ForegroundColor Yellow
        Write-Host '  1) 安装 Windows SDK（含 Windows SDK Signing Tools / mt.exe），或' -ForegroundColor Yellow
        Write-Host '  2) 加 -SkipManifest 直接运行（可能复现 0xc0000139 STATUS_ENTRYPOINT_NOT_FOUND）。' -ForegroundColor Yellow
        throw '缺少 mt.exe，无法给测试副本嵌入清单。'
    }

    if (-not (Test-Path -LiteralPath $workRoot)) {
        New-Item -ItemType Directory -Path $workRoot -Force | Out-Null
    }
    if (-not $SkipManifest) {
        Set-Content -LiteralPath $manifestFile -Value $commonControlsManifest -Encoding UTF8
    }

    Write-Host '编译测试目标（不运行）…' -ForegroundColor Cyan
    $cargoArgs = @('test', '--manifest-path', $manifestPath, '--locked', '--no-run', '--message-format=json', '-j', '1', '--lib')
    if ($Offline) { $cargoArgs += '--offline' }
    # Windows PowerShell 5.1 treats ordinary native stderr as NativeCommandError.
    # Keep stdout as JSON lines; use the actual process exit code for failure.
    $stderrLog = Join-Path $workRoot 'cargo-test-stderr.log'
    $ErrorActionPreference = 'Continue'
    $jsonLines = & cargo @cargoArgs 2> $stderrLog
    $cargoExit = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    Get-Content -LiteralPath $stderrLog -Tail 8 | Out-Host
    $jsonLines | Set-Content -LiteralPath $testLog -Encoding UTF8
    if ($cargoExit -ne 0) {
        Write-Host ('cargo test --no-run 失败（退出码 {0}），完整输出：{1}' -f $cargoExit, $testLog) -ForegroundColor Red
        exit $cargoExit
    }

    $executables = @()
    foreach ($line in $jsonLines) {
        $text = $line.ToString().Trim()
        if (-not $text.StartsWith('{')) { continue }
        try { $message = $text | ConvertFrom-Json } catch { continue }
        if ($message.reason -ne 'compiler-artifact') { continue }
        if (-not $message.executable) { continue }
        if ($message.profile.test -ne $true) { continue }
        $executables += $message.executable
    }

    if ($executables.Count -eq 0) {
        Write-Host ('没有找到测试可执行文件，完整输出：{0}' -f $testLog) -ForegroundColor Red
        exit 3
    }

    $failed = 0
    foreach ($exe in $executables) {
        $name = [System.IO.Path]::GetFileNameWithoutExtension($exe)
        $copy = Join-Path $workRoot ('{0}-manifested.exe' -f $name)
        Copy-Item -LiteralPath $exe -Destination $copy -Force

        if (-not $SkipManifest) {
            & $mt -nologo -manifest $manifestFile ('-outputresource:{0};#1' -f $copy) | Out-Host
            if ($LASTEXITCODE -ne 0) {
                Write-Host ('mt.exe 嵌入清单失败（退出码 {0}）：{1}' -f $LASTEXITCODE, $copy) -ForegroundColor Red
                $failed++
                continue
            }
        }

        $testArgs = @('--test-threads=1')
        if ($Filter) { $testArgs = @($Filter) + $testArgs }
        Write-Host ('运行测试副本：{0} {1}' -f $copy, ($testArgs -join ' ')) -ForegroundColor Cyan
        & $copy @testArgs | Out-Host
        $code = $LASTEXITCODE
        if ($code -ne 0) {
            Write-Host ('测试失败（退出码 {0}）：{1}' -f $code, $copy) -ForegroundColor Red
            $failed++
        }
    }

    if ($failed -gt 0) {
        Write-Host ('有 {0} 个测试程序未通过。' -f $failed) -ForegroundColor Red
        exit 1
    }
    Write-Host '全部测试程序通过。' -ForegroundColor Green
    exit 0
} catch {
    Write-Host ('运行失败：{0}' -f $_.Exception.Message) -ForegroundColor Red
    exit 1
}
