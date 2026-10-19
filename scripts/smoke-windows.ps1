# 安装包冒烟测试：静默安装 → 检查 Run 键 → 启动并确认是安装版 → 静默卸载 → 检查清理。
# 只在当前用户的桌面会话里有效；SSH 会话里托盘和 webview 起不来。
param(
    [Parameter(Mandatory)][string]$Installer,
    [int]$StartupSecs = 15
)

$ErrorActionPreference = 'Stop'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$logPath = Join-Path $env:APPDATA 'DeskBaton\logs\deskbaton.log'

function Fail([string]$msg) {
    Write-Host "::error::$msg"
    if (Test-Path $logPath) {
        Write-Host '--- deskbaton.log ---'
        Get-Content $logPath -Encoding UTF8 | Write-Host
    }
    Get-Process -Name DeskBaton -ErrorAction SilentlyContinue | Stop-Process -Force
    exit 1
}

function DumpInstall {
    Write-Host '--- 卸载注册表项 ---'
    Get-ChildItem 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall' -ErrorAction SilentlyContinue |
        Get-ItemProperty | Where-Object { $_.DisplayName -like '*DeskBaton*' } |
        Format-List DisplayName, DisplayVersion, InstallLocation, UninstallString | Out-String | Write-Host
    $dir = Join-Path $env:LOCALAPPDATA 'DeskBaton'
    Write-Host "--- $dir ---"
    if (Test-Path $dir) { Get-ChildItem $dir | Format-Table Name, Length | Out-String | Write-Host }
    Write-Host '--- LOCALAPPDATA 下的 DeskBaton*.exe ---'
    Get-ChildItem $env:LOCALAPPDATA -Recurse -Depth 3 -Filter 'DeskBaton*.exe' -ErrorAction SilentlyContinue |
        ForEach-Object { Write-Host $_.FullName }
}

function RunValue {
    (Get-ItemProperty -Path $runKey -Name DeskBaton -ErrorAction SilentlyContinue).DeskBaton
}

function WaitUntil([scriptblock]$cond, [int]$secs) {
    for ($i = 0; $i -lt $secs; $i++) {
        if (& $cond) { return $true }
        Start-Sleep -Seconds 1
    }
    return [bool](& $cond)
}

$Installer = (Resolve-Path $Installer).Path
Write-Host "安装：$Installer"
$p = Start-Process -FilePath $Installer -ArgumentList '/S' -Wait -PassThru
if ($p.ExitCode -ne 0) { Fail "安装器退出码 $($p.ExitCode)" }

$run = RunValue
if (-not $run) { Fail '安装后 HKCU Run 键缺少 DeskBaton' }
$exe = $run.Trim('"')
if (-not (WaitUntil { Test-Path $exe } 30)) {
    DumpInstall
    Fail "Run 键指向的程序不存在：$exe"
}
$installDir = Split-Path $exe -Parent
Write-Host "已安装：$exe"

if (Test-Path $logPath) { Remove-Item $logPath -Force }
$proc = Start-Process -FilePath $exe -PassThru
Start-Sleep -Seconds $StartupSecs
if ($proc.HasExited) { Fail "启动后 $StartupSecs 秒内退出，退出码 $($proc.ExitCode)" }
if (-not (Test-Path $logPath)) { Fail "未生成日志：$logPath" }
$log = Get-Content $logPath -Raw -Encoding UTF8
Write-Host '--- deskbaton.log ---'
Write-Host $log
if ($log -match 'panicked') { Fail '日志中有 panic' }
if ($log -notmatch '安装版=true') { Fail '程序未判定为安装版' }

Stop-Process -Id $proc.Id -Force
if (-not (WaitUntil { $proc.HasExited } 10)) { Fail '无法结束 DeskBaton 进程' }

$uninstaller = Join-Path $installDir 'uninstall.exe'
if (-not (Test-Path $uninstaller)) { Fail "找不到卸载程序：$uninstaller" }
Start-Process -FilePath $uninstaller -ArgumentList '/S' -Wait | Out-Null
if (-not (WaitUntil { -not (RunValue) } 30)) { Fail '卸载后 HKCU Run 键仍有 DeskBaton' }
if (-not (WaitUntil { -not (Test-Path $exe) } 30)) { Fail "卸载后程序仍在：$exe" }

Write-Host '冒烟测试通过'
