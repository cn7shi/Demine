use anyhow::{Context, Result, bail};
use clap::Parser;
use demine::{
    collector::{Collector, ScanReport, normalized_path},
    server::{AppState, collect_loop, router},
    store::Store,
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Parser)]
#[command(
    version,
    about = "Demine — 自动记录 Agent 工作过程，在本地展开执行证据"
)]
struct Args {
    /// 要观察的项目目录（包含其子目录中的会话）
    #[arg(long, default_value = ".")]
    project: PathBuf,
    /// Codex sessions 目录，默认 $CODEX_HOME/sessions 或 ~/.codex/sessions
    #[arg(long)]
    sessions_dir: Option<PathBuf>,
    /// 本地数据库目录，默认项目下的 .demine
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// 本地浏览器端口；只监听 127.0.0.1
    #[arg(long, default_value_t = 4317)]
    port: u16,
    #[arg(long,default_value_t=2,value_parser=clap::value_parser!(u64).range(1..=60))]
    poll_seconds: u64,
    /// 扫描一次并打印计数，不启动界面
    #[arg(long)]
    once: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let project = args
        .project
        .canonicalize()
        .context("项目目录不存在或无法访问")?;
    if !project.is_dir() {
        bail!("--project 必须是目录");
    }
    let sessions_root = match args.sessions_dir {
        Some(path) => path,
        None => {
            let base = if let Some(path) = std::env::var_os("CODEX_HOME") {
                PathBuf::from(path)
            } else {
                PathBuf::from(
                    std::env::var_os("USERPROFILE")
                        .or_else(|| std::env::var_os("HOME"))
                        .context("无法确定用户目录，请指定 --sessions-dir")?,
                )
                .join(".codex")
            };
            base.join("sessions")
        }
    };
    if !sessions_root.is_dir() {
        bail!("执行记录目录不存在，请使用 --sessions-dir 指定 Codex sessions 目录");
    }
    // Stable absolute source keys prevent duplicate ingestion after changing cwd.
    let sessions_root = sessions_root
        .canonicalize()
        .context("无法定位执行记录目录")?;
    let data_dir = args.data_dir.unwrap_or_else(|| project.join(".demine"));
    std::fs::create_dir_all(&data_dir).context("无法创建数据目录")?;
    let project_key = normalized_path(&project);
    let mut store = Store::open(&data_dir.join("demine.sqlite3"), &project_key)?;
    let collector = Collector {
        project: project.clone(),
        sessions_root: sessions_root.clone(),
    };
    if args.once {
        let report = collector.scan(&mut store);
        println!("{}", serde_json::to_string_pretty(&report)?);
        if !report.errors.is_empty() {
            bail!(
                "扫描完成但有 {} 项错误，请查看上方报告",
                report.errors.len()
            );
        }
        return Ok(());
    }
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, args.port))
        .await
        .context("无法监听本地端口，可使用 --port 更换端口")?;
    let port = listener.local_addr()?.port();
    let state = Arc::new(AppState {
        store: Mutex::new(store),
        report: Mutex::new(ScanReport::default()),
        project: project_key,
        sessions_root: sessions_root.to_string_lossy().into_owned(),
        poll_seconds: args.poll_seconds,
        port,
    });
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn(collect_loop(state.clone(), collector, stop_rx));
    println!(
        "Demine 已启动：http://127.0.0.1:{port}\n正在观察：{}\n记录只保存在本机。按 Ctrl+C 停止。",
        project.display()
    );
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    let _ = stop_tx.send(true);
    worker.await.context("等待采集任务结束失败")?;
    Ok(())
}
