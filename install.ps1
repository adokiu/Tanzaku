# Tanzaku 一键安装（Windows）。以管理员身份运行 PowerShell：
#
#   powershell -ExecutionPolicy Bypass -Command "& ([scriptblock]::Create((irm https://raw.githubusercontent.com/adokiu/Tanzaku/main/install.ps1))) -Endpoint 'wss://board.example.com:9001' -Token 'TOKEN'"
#
# 以开机启动的计划任务（SYSTEM 账户）运行，异常退出每分钟自动重启。
param(
    [string]$Endpoint = "",
    [string]$Token = "",
    [ValidateSet("client", "server")]
    [string]$Role = "client",
    [string]$Version = "",
    [Alias("GithubProxy")][string]$GhProxy = "",
    [string]$InstallDir = "",
    [string]$ServiceName = "",
    [ValidateSet("error", "warn", "info", "debug", "trace")]
    [string]$LogLevel = "info",
    [string]$Repo = "adokiu/Tanzaku",
    [switch]$Uninstall
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

function Info($message) { Write-Host ">>> $message" -ForegroundColor Green }
function Fail($message) { Write-Host "[错误] $message" -ForegroundColor Red; exit 1 }

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Fail "需要以管理员身份运行 PowerShell"
}

$binName = "tanzaku-$Role"
if (-not $InstallDir) { $InstallDir = Join-Path $env:ProgramFiles "Tanzaku\$Role" }
if (-not $ServiceName) { $ServiceName = "tanzaku-$Role" }
if ($ServiceName -notmatch '^[A-Za-z0-9._-]+$') { Fail "服务名称只能包含字母、数字、. _ -" }
$binPath = Join-Path $InstallDir "$binName.exe"
$configPath = Join-Path $InstallDir "$Role.toml"
$logPath = Join-Path $InstallDir "$ServiceName.log"

function Stop-Existing {
    $task = Get-ScheduledTask -TaskName $ServiceName -ErrorAction SilentlyContinue
    if ($task) {
        Stop-ScheduledTask -TaskName $ServiceName -ErrorAction SilentlyContinue
        Unregister-ScheduledTask -TaskName $ServiceName -Confirm:$false
    }
    Get-Process -Name $binName -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq $binPath } |
        Stop-Process -Force -ErrorAction SilentlyContinue
}

if ($Uninstall) {
    Info "卸载 $ServiceName"
    Stop-Existing
    if (Test-Path $InstallDir) { Remove-Item -Recurse -Force $InstallDir }
    Info "已卸载"
    exit 0
}

if (-not $Endpoint) { Fail "缺少 -Endpoint" }
# 控制通道是 WebSocket：https -> wss，http -> ws，未写协议默认 ws。
$Endpoint = $Endpoint.Trim().TrimEnd('/')
if ($Endpoint -match '^(?i)https://') { $Endpoint = 'wss://' + $Endpoint.Substring(8) }
elseif ($Endpoint -match '^(?i)http://') { $Endpoint = 'ws://' + $Endpoint.Substring(7) }
elseif ($Endpoint -notmatch '^(?i)wss?://') { $Endpoint = 'ws://' + $Endpoint }
if (-not $Token) { Fail "缺少 -Token" }

$arch = $env:PROCESSOR_ARCHITEW6432
if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
switch ($arch) {
    "AMD64" { $archName = "amd64" }
    "x86" { $archName = "386" }
    "ARM64" { $archName = "arm64" }
    default { Fail "不支持的 CPU 架构: $arch" }
}

$asset = "$binName-windows-$archName.exe"
if ($Version) {
    $url = "https://github.com/$Repo/releases/download/$Version/$asset"
} else {
    $url = "https://github.com/$Repo/releases/latest/download/$asset"
}
if ($GhProxy) {
    if ($GhProxy -notmatch '^https?://') { $GhProxy = "https://$GhProxy" }
    $url = $GhProxy.TrimEnd('/') + "/" + $url
}

Info "系统: windows/$archName  角色: $Role"
Info "下载 $url"
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
$tmp = Join-Path $InstallDir ".$binName.download"
try {
    Invoke-WebRequest -Uri $url -OutFile $tmp -UseBasicParsing
} catch {
    Fail "下载失败，可尝试 -GhProxy 或 -Version：$($_.Exception.Message)"
}

Stop-Existing
Move-Item -Force $tmp $binPath

function Escape-Toml($value) { return $value.Replace('\', '\\').Replace('"', '\"') }
$config = "# 由 install.ps1 生成`r`nboard = `"$(Escape-Toml $Endpoint)`"`r`ntoken = `"$(Escape-Toml $Token)`"`r`n"
[IO.File]::WriteAllText($configPath, $config, (New-Object Text.UTF8Encoding($false)))
icacls $configPath /inheritance:r /grant:r "SYSTEM:(F)" "Administrators:(F)" | Out-Null

$command = "set RUST_LOG=$LogLevel&& `"$binPath`" -c `"$configPath`" >> `"$logPath`" 2>&1"
$action = New-ScheduledTaskAction -Execute "cmd.exe" -Argument "/c $command" -WorkingDirectory $InstallDir
$trigger = New-ScheduledTaskTrigger -AtStartup
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -RestartCount 9999 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit ([TimeSpan]::Zero) -StartWhenAvailable
$taskPrincipal = New-ScheduledTaskPrincipal -UserId "SYSTEM" -LogonType ServiceAccount -RunLevel Highest
Register-ScheduledTask -TaskName $ServiceName -Action $action -Trigger $trigger -Settings $settings `
    -Principal $taskPrincipal -Description "Tanzaku $Role" -Force | Out-Null
Start-ScheduledTask -TaskName $ServiceName

Info "安装完成: $binPath"
Info "配置文件: $configPath"
Info "日志: $logPath"
Info "卸载: 重新执行本脚本并加 -Uninstall -Role $Role -ServiceName $ServiceName"
