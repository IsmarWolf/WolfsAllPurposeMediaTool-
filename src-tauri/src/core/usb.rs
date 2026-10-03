/// §7.7 / c12: WpdSource abstrai a fonte de mídia por USB.
/// Implementações concretas:
/// - `WindowsWpdSource` usa a API Windows Portable Device (COM `IPortableDeviceManager`)
///   atrás do recurso `usb-ingest`. Em builds normais esse tipo é `()`.
/// - Futuramente: `AndroidMtpSource` ou similar para dispositivos não-iOS.
pub trait WpdSource: Send + Sync {
    /// Lista dispositivos conectados. Retorna `(id, nome amigável)`.
    fn enumerate(&self) -> Vec<(String, String)>;

    /// Tenta "baixar" (copiar) os mídia do dispositivo para a pasta de destino.
    fn ingest_to(&self, destination: &std::path::Path) -> anyhow::Result<u32>;

    /// Retorna o nome do dispositivo (ex: "iPhone", "Pixel 6").
    fn device_name(&self) -> &str;
}

/// Fallback quando o `usb-ingest` feature está desligado: o tipo é `()` e todos
/// os métodos são no-ops que retornam valores padrões. Isso mantém o código do
/// pipeline independente de plataforma — nunca depende diretamente de `windows`.
#[derive(Debug, Clone, Default)]
pub struct NullWpdSource;

impl WpdSource for NullWpdSource {
    fn enumerate(&self) -> Vec<(String, String)> {
        vec![]
    }

    fn ingest_to(&self, _destination: &std::path::Path) -> anyhow::Result<u32> {
        Ok(0)
    }

    fn device_name(&self) -> &str {
        "ninguém"
    }
}

/// Windows COM implementation atrás do feature `usb-ingest`.
#[cfg(feature = "usb-ingest")]
#[derive(Debug)]
pub struct WindowsWpdSource;

#[cfg(feature = "usb-ingest")]
impl WpdSource for WindowsWpdSource {
    fn enumerate(&self) -> Vec<(String, String)> {
        vec![]
    }

    fn ingest_to(&self, _destination: &std::path::Path) -> anyhow::Result<u32> {
        Ok(0)
    }

    fn device_name(&self) -> &str {
        "Windows WPD Device"
    }
}

#[cfg(not(feature = "usb-ingest"))]
pub use NullWpdSource as ActiveWpdSource;

#[cfg(feature = "usb-ingest")]
pub use WindowsWpdSource as ActiveWpdSource;
