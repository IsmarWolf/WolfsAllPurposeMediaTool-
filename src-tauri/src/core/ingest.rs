/// §12.1 / c12: Coordenador de ingestão que orquestra Wi-Fi, USB e fontes locais.
use crate::core::models::ScanSummary;
use crate::core::usb::{ActiveWpdSource, WpdSource};

/// Fonte de mídia para o coordinator. USB (c12) e pasta local (Disco local).
#[derive(Debug, Clone)]
pub enum LocalSource {
    Usb(ActiveWpdSource),
    Folder(std::path::PathBuf),
}

impl LocalSource {
    fn device_name(&self) -> String {
        match self {
            LocalSource::Usb(s) => s.device_name().into(),
            LocalSource::Folder(_) => "Disco local".into(),
        }
    }
}

/// Coordenador de ingestão - estrutura mínima para c12.
pub struct IngestCoordinator {
    source: LocalSource,
}

impl IngestCoordinator {
    pub fn new(source: LocalSource) -> Self {
        Self { source }
    }

    /// Retorna o ScanSummary final (placeholder - implementação completa virá depois).
    pub fn start(&self) -> ScanSummary {
        let device = self.source.device_name();
        ScanSummary {
            device,
            found: 0,
            inserted: 0,
            duplicates: 0,
            organized: 0,
            no_metadata: 0,
            repaired: 0,
            located: 0,
            located_none: 0,
            skipped: 0,
            cancelled: false,
        }
    }
}
