/// `kern.wakereason` 是否表示网络（Wi-Fi / 魔术包）唤醒。`rtc` 定时暗唤醒不含此前缀。
pub fn is_network_wake_reason(reason: &str) -> bool {
    reason.contains("wifibt")
}

#[cfg(test)]
mod tests {
    use super::is_network_wake_reason;

    #[test]
    fn wifi_magic_packet_counts() {
        assert!(is_network_wake_reason(
            "smc.70070000 wifibt SMC.OutboxNotEmpty nub-spmi-a0.0x0B"
        ));
    }

    #[test]
    fn rtc_maintenance_does_not() {
        assert!(!is_network_wake_reason("rtc"));
        assert!(!is_network_wake_reason(""));
    }
}
