use async_channel::{unbounded, Receiver, Sender};
use async_fs::{File, OpenOptions};
use async_std::{
    io::{self, BufWriter, ReadExt, SeekExt, WriteExt},
    path::PathBuf,
    sync::Mutex,
};
use bevy::{prelude::*, tasks::IoTaskPool};
use serde::{Deserialize, Serialize};
use std::{io::SeekFrom, marker::PhantomData, sync::Arc};

#[derive(Resource, Clone, Deref, DerefMut)]
pub struct IoSender<R>(Sender<R>);

#[derive(Resource)]
struct IoSinkTaskData<R, W> {
    rx: Receiver<R>,
    writer: Arc<Mutex<W>>,
}

struct IoSinkPlugin<R, W> {
    writer: Arc<Mutex<W>>,
    _phantom: PhantomData<R>,
}

impl<R, W> IoSinkPlugin<R, W> {
    fn new(writer: W) -> Self {
        Self {
            writer: Arc::new(Mutex::new(writer)),
            _phantom: PhantomData,
        }
    }
}

impl<R, W> Plugin for IoSinkPlugin<R, W>
where
    R: Resource + Send + Sync + 'static,
    W: IoWriter<R> + Send + Sync + 'static,
{
    fn build(&self, app: &mut App) {
        let (tx, rx): (Sender<R>, Receiver<R>) = unbounded();

        app.insert_resource(IoSender(tx));

        app.insert_resource(IoSinkTaskData {
            rx,
            writer: self.writer.clone(),
        });

        app.add_systems(Startup, spawn_io_sink_task::<R, W>);
    }
}

fn spawn_io_sink_task<R, W>(task_data: Res<IoSinkTaskData<R, W>>)
where
    R: Send + 'static,
    W: IoWriter<R> + Send + Sync + 'static,
{
    let rx = task_data.rx.clone();
    let writer = task_data.writer.clone();

    IoTaskPool::get()
        .spawn(async move {
            let mut writer_lock = writer.lock().await;
            if let Err(e) = writer_lock.init().await {
                error!("{}", e);
            }

            while let Ok(msg) = rx.recv().await {
                if let Err(e) = writer_lock.write(msg).await {
                    error!("{}", e);
                }
            }

            if let Err(e) = writer_lock.close().await {
                error!("{}", e);
            }
        })
        .detach();
}

pub trait IoWriter<R>: Send + Sync + 'static {
    fn init(&mut self) -> impl std::future::Future<Output = io::Result<()>> + Send {
        async { Ok(()) }
    }

    fn write(&mut self, data: R) -> impl std::future::Future<Output = io::Result<()>> + Send;

    fn flush(&mut self) -> impl std::future::Future<Output = io::Result<()>> + Send {
        async { Ok(()) }
    }

    fn close(&mut self) -> impl std::future::Future<Output = io::Result<()>> + Send {
        async { Ok(()) }
    }
}

pub struct FileSink<R> {
    path: PathBuf,
    writer: Option<BufWriter<File>>,
    _marker: PhantomData<R>,
}

impl<R> FileSink<R> {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            writer: None,
            _marker: PhantomData,
        }
    }
}

impl<R> IoWriter<R> for FileSink<R>
where
    R: Serialize + Send + Sync + 'static,
{
    async fn init(&mut self) -> io::Result<()> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .append(false)
            .open(&self.path)
            .await?;
        self.writer = Some(BufWriter::with_capacity(64 * 1024, file));
        Ok(())
    }

    async fn write(&mut self, data: R) -> io::Result<()> {
        let json =
            serde_json::to_vec(&data).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let writer = self.writer.as_mut().expect("FileSink::init not called");

        writer.seek(SeekFrom::Start(0)).await?;
        writer.write_all(&json).await?;
        writer.get_mut().set_len(json.len() as u64).await?;

        writer.flush().await
    }
}
pub struct FileSinkPlugin<R> {
    /// If true, the resource will be synced to disk on every change.
    sync_res: bool,
    path: PathBuf,
    _phantom: PhantomData<R>,
}

impl<R> FileSinkPlugin<R> {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            _phantom: PhantomData,
            sync_res: false,
        }
    }
}

pub struct AutoSave {
    pub enabled: bool,
    pub timer: Timer,
}

impl AutoSave {
    pub fn from_timer(timer: Timer) -> Self {
        Self {
            enabled: true,
            timer,
        }
    }
}

#[derive(Resource)]
struct LoadFileReceiver<R>(Receiver<R>);

impl<R> Plugin for FileSinkPlugin<R>
where
    R: for<'de> Deserialize<'de> + Clone + Serialize + Resource + Default + Send + Sync + 'static,
{
    fn build(&self, app: &mut App) {
        let path = self.path.clone();
        let (tx, rx): (Sender<R>, Receiver<R>) = unbounded();

        let file_sink = FileSink::<R>::new(self.path.clone());
        app.add_plugins(IoSinkPlugin::<R, FileSink<R>>::new(file_sink));

        app.insert_resource(LoadFileReceiver(rx));

        app.add_systems(
            FixedUpdate,
            |mut commands: Commands, load_file_recv: Res<LoadFileReceiver<R>>| {
                if let Ok(msg) = load_file_recv.0.try_recv() {
                    commands.insert_resource(msg);
                }
            },
        );
        if self.sync_res {
            app.add_systems(
                Update,
                sync_file::<R>.run_if(resource_exists_and_changed::<R>),
            );
        }
        app.add_systems(Startup, move || {
            let path = path.clone();
            let tx = tx.clone();
            IoTaskPool::get()
                .spawn(async move {
                    let mut file = match OpenOptions::new()
                        .create(true)
                        .read(true)
                        .write(true)
                        .append(false)
                        .open(&path)
                        .await
                    {
                        Ok(file) => file,
                        Err(e) => {
                            error!("{e}");
                            return Ok::<(), io::Error>(());
                        }
                    };

                    let mut buf = String::new();
                    if let Err(e) = file.read_to_string(&mut buf).await {
                        error!("{e}");
                        return Ok::<(), io::Error>(());
                    }

                    let msg: R = serde_json::from_str(&buf).unwrap_or_default();
                    if let Err(e) = tx.send(msg).await {
                        error!("{e}");
                    }
                    Ok::<(), io::Error>(())
                })
                .detach();
        });
    }
}

fn sync_file<R>(sender: Res<IoSender<R>>, res: Res<R>)
where
    R: for<'de> Deserialize<'de> + Clone + Serialize + Resource + Default + Send + Sync + 'static,
{
    if let Err(err) = sender.try_send(res.clone()) {
        error!("{err}");
    }
}
