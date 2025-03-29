mod display;
mod task;

use std::{
    cmp::max, collections::HashMap, io, path::PathBuf, sync::atomic::Ordering, time::Duration,
};

use display::DisplayManager;
use log::{debug, info};
use task::{RefreshTaskBuilder, RefreshTaskStatus};

use crate::manifest::{Manifest, ManifestSource};

#[derive(Debug)]
pub struct RefreshOptions {
    pub dirpath: PathBuf,
    pub manifest: Option<PathBuf>,
    pub chunk_size: usize,
    pub max_workers: usize,
    pub debug: bool,
    pub no_display: bool,
    pub no_progress: bool,
    pub verbosity: u8,
}

#[derive(Debug, thiserror::Error)]
pub enum RefreshError {
    #[error("{0}")]
    IoError(#[from] std::io::Error),

    #[error("{0}")]
    ManifestError(#[from] crate::manifest::ManifestError),

    #[error("Failed to join checksum generation task, {0}")]
    TaskJoinFailure(#[from] tokio::task::JoinError),

    #[error("Unknown error occurred, {0}")]
    Unknown(#[from] anyhow::Error),
}

pub type RefreshResult<T> = Result<T, RefreshError>;

pub async fn refresh(options: RefreshOptions) -> RefreshResult<()> {
    debug!("{:?}", options);
    if !options.dirpath.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("No directory exists at {:?}", options.dirpath),
        )
        .into());
    }

    let dirpath = options.dirpath.clone();
    let manifest_source = if let Some(manifest_filepath) = options.manifest {
        ManifestSource::from_path(&manifest_filepath).ok_or(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Invalid manifest file format found at {:?}",
                manifest_filepath
            ),
        ))?
    } else {
        ManifestSource::from_path(&options.dirpath).ok_or(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "No valid manifest file format found in directory {:?}",
                dirpath
            ),
        ))?
    };

    let manifest_filepath = manifest_source.filepath.clone();
    let manifest_parser = manifest_source.parser();
    let manifest = manifest_parser.parse(&manifest_source).await?;
    let mut refresh_tasks = Vec::with_capacity(manifest.artifacts.len());
    let refresh_task_builder = RefreshTaskBuilder::new(options.max_workers, options.chunk_size);

    let mut display_manager = DisplayManager::new(
        max(
            1024,
            options.max_workers * 8 + (options.max_workers.saturating_sub(4) * 4),
        ),
        &refresh_task_builder.counters,
        manifest.artifacts.len(),
        options.verbosity,
        options.no_display || options.debug,
    );

    display_manager.report_start(manifest_source).await?;
    if !options.no_progress && !options.debug {
        display_manager.start_progress_worker().await?;
    }

    for (filename, old) in &manifest.artifacts {
        // TODO: allow the user to override
        refresh_tasks.push(refresh_task_builder.refresh_checksum(
            &PathBuf::from(filename),
            old,
            None,
            None,
        ));
    }

    let mut artifacts = HashMap::new();
    for task in refresh_tasks {
        let task_result = task.await?;
        match task_result {
            Ok(result) => {
                let filename = result.filename.clone();
                let status = result.status.clone();
                match status {
                    RefreshTaskStatus::Removed => (),
                    RefreshTaskStatus::Updated { old: _, new } => {
                        artifacts.insert(filename, new);
                    }
                    RefreshTaskStatus::Unchanged { checksum } => {
                        artifacts.insert(filename, checksum);
                    }
                };

                display_manager.report_task_result(result).await?;
            }
            Err(error) => display_manager.report_task_error(error).await?,
        }
    }

    info!("Writing manifest to {:?}", manifest_filepath);
    tokio::fs::write(
        manifest_filepath,
        manifest_parser
            .to_string(&Manifest {
                version: None,
                artifacts,
            })
            .await?,
    )
    .await?;

    display_manager.report_progress(true).await?;
    display_manager.stop_progress_worker().await;

    tokio::time::sleep(Duration::from_millis(10)).await;
    let (sync_tx, sync_rx) = tokio::sync::oneshot::channel::<()>();
    display_manager.report_exit(sync_tx).await?;
    sync_rx.await.unwrap();

    if refresh_task_builder.counters.error.load(Ordering::Relaxed) > 0 {
        std::process::exit(1);
    }

    Ok(())
}
