# 由 scripts/probe/mac-wol-wake.sh 经临时 HTTP 服务下发，下发时在开头注入 $Mac、$Ip、$Broadcast、$WaitSec。
# 在 Windows PowerShell 里运行：irm http://<Mac 的 IP>:8765 | iex

function Stamp { (Get-Date).ToString('HH:mm:ss') }

function Test-Mac {
    try { (New-Object System.Net.NetworkInformation.Ping).Send($Ip, 500).Status -eq 'Success' } catch { $false }
}

$macBytes = $Mac -split '[:-]' | ForEach-Object { [Convert]::ToByte($_, 16) }
$packet = [byte[]](@(0xFF) * 6 + $macBytes * 16)

Write-Host "$(Stamp) Mac 约 10 秒后睡眠。$WaitSec 秒后发魔术包，期间每 5 秒 ping 一次 $Ip（只作记录）"
$pingBefore = $false
for ($left = $WaitSec; $left -gt 0; $left -= 5) {
    Start-Sleep -Seconds ([Math]::Min(5, $left))
    $pingBefore = Test-Mac
    Write-Host "$(Stamp) 还剩 $([Math]::Max(0, $left - 5)) 秒，ping：$(if ($pingBefore) { '通' } else { '不通' })"
}

$targets = @('255.255.255.255', $Broadcast, $Ip) | Where-Object { $_ } | Select-Object -Unique
$udp = New-Object System.Net.Sockets.UdpClient
$udp.EnableBroadcast = $true
foreach ($round in 1..3) {
    foreach ($target in $targets) {
        try { [void]$udp.Send($packet, $packet.Length, $target, 9) } catch { Write-Host "$(Stamp) 发往 $target 失败：$($_.Exception.Message)" }
    }
    Start-Sleep -Milliseconds 500
}
$udp.Close()
Write-Host "$(Stamp) 已发魔术包：目标 MAC $Mac，发往 $($targets -join '、') 的 UDP 9 端口，各 3 次"

$wokeAfter = $null
for ($s = 1; $s -le 30; $s++) {
    if (Test-Mac) { $wokeAfter = $s; break }
    Start-Sleep -Seconds 1
}
if ($pingBefore) {
    Write-Host "$(Stamp) Mac 睡眠中也能 ping 通，ping 判断不了是否醒来，以 Mac 端结论为准"
} elseif ($wokeAfter) {
    Write-Host "$(Stamp) 发包后约 $wokeAfter 秒 ping 通，Mac 很可能已被唤醒"
} else {
    Write-Host "$(Stamp) 30 秒内 ping 不通（Mac 没醒，或开了防火墙隐身模式）"
}
Write-Host "$(Stamp) 现在用 OSD 切回 USB-C。30 秒后还没画面就开盖，Mac 屏幕上会显示结论"
