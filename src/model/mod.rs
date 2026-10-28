//! Model 层：只负责“事实”，不关心怎么画。
pub mod sampler;

use std::time::Instant;

#[derive(Clone, Debug)]
pub struct ServiceState {
    pub name: String,
    pub ok: bool,
}

/// 网络：只关心“怎么连的、地址是多少”
#[derive(Clone, Debug)]
pub struct NetState {
    /// "有线" / "WiFi"
    pub kind: &'static str,
    pub iface: String,
    /// 有地址就给，没连上就是 None（网线拔了也照样占一行，标红）
    pub ipv4: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SystemState {
    pub stamp: Instant,
    pub host: String,
    pub kernel: String,
    pub cpu_pct: f32,
    pub mem_used_gb: f32,
    pub mem_total_gb: f32,
    pub cpu_temp_c: f32,
    pub load1: f32,
    pub cores: u32,
    pub services: Vec<ServiceState>,
    /// 有线 / WiFi：物理网口各占一条，没地址的也在，用来显示“未连接”
    pub net: Vec<NetState>,
}

impl SystemState {
    pub fn age_ms(&self) -> u128 {
        self.stamp.elapsed().as_millis()
    }
}
