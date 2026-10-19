#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("当前平台或模式不支持：{0}")]
    Unsupported(&'static str),
    #[error("DDC 通信失败：{0}")]
    Ddc(String),
    #[error("电源控制失败：{0}")]
    Power(String),
    #[error("唤醒失败：{0}")]
    Wake(String),
    #[error("键鼠路由失败：{0}")]
    Input(String),
    #[error("网络错误：{0}")]
    Net(String),
    #[error("配置错误：{0}")]
    Config(String),
    #[error("剪贴板失败：{0}")]
    Clipboard(String),
    #[error("文件传输失败：{0}")]
    File(String),
    #[error("切换超时")]
    Timeout,
}

pub type Result<T> = std::result::Result<T, Error>;
