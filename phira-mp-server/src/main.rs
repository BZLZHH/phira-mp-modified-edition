mod l10n;

mod room;
pub use room::*;

mod server;
pub use server::*;

mod session;
pub use session::*;

use anyhow::Result;
use std::{
    collections::{
        hash_map::{Entry, VacantEntry},
        HashMap,
    },
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    path::Path,
};
use tokio::{net::TcpListener, sync::RwLock};
use tracing::warn;
use tracing_appender::non_blocking::WorkerGuard;
use uuid::Uuid;
use std::io::{self, Write};
use std::fs::File;
use std::thread;
use std::fs;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use crate::{Room, User};

pub type SafeMap<K, V> = RwLock<HashMap<K, V>>;
pub type IdMap<V> = SafeMap<Uuid, V>;

fn vacant_entry<V>(map: &mut HashMap<Uuid, V>) -> VacantEntry<'_, Uuid, V> {
    let mut id = Uuid::new_v4();
    while map.contains_key(&id) {
        id = Uuid::new_v4();
    }
    match map.entry(id) {
        Entry::Vacant(entry) => entry,
        _ => unreachable!(),
    }
}

pub fn init_log(file: &str) -> Result<WorkerGuard> {
    use tracing::{metadata::LevelFilter, Level};
    use tracing_log::LogTracer;
    use tracing_subscriber::{filter, fmt, prelude::*, EnvFilter};

    let log_dir = Path::new("log");
    if log_dir.exists() {
        if !log_dir.is_dir() {
            panic!("log exists and is not a folder");
        }
    } else {
        std::fs::create_dir(log_dir).expect("failed to create log folder");
    }

    LogTracer::init()?;

    let (non_blocking, guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::hourly(log_dir, file));

    let subscriber = tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_writer(non_blocking)
                .with_filter(LevelFilter::DEBUG),
        )
        .with(
            fmt::layer()
                .with_writer(std::io::stdout)
                .with_filter(EnvFilter::from_default_env()),
        )
        .with(
            filter::Targets::new()
                .with_target("hyper", Level::INFO)
                .with_target("rustls", Level::INFO)
                .with_target("isahc", Level::INFO)
                .with_default(Level::TRACE),
        );

    tracing::subscriber::set_global_default(subscriber).expect("unable to set global subscriber");
    Ok(guard)
}

#[tokio::main]
async fn main() -> Result<()> {
    let _guard = init_log("phira-mp")?;

    let port = 666;
	let pmps_port = 667;
	
    let addrs: &[SocketAddr] = &[
        SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), port),
        SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), port),
    ];
	
	let pmps_file_name = "pmps.txt";
	let prs_file_name = "public_rooms.json";
	let mut prs_file = File::create(prs_file_name)?;
	prs_file.write_all(b"[]")?;
	
	let pmps_file_path = Path::new(pmps_file_name);
	let mut pmps_file_str : String = "false".to_string();
    if pmps_file_path.exists() {
		let pmps_file_str = fs::read_to_string(pmps_file_name)?;
    } else {
        print!("检测到您是第一次使用(或未配置)此服务端\n请确定您是否愿意公开分享此服务器到pmpl.bzlzhh.top网站(公开网站)\n\n若您同意,以下内容将会被允许获取:\n1.安全的服务器地址(若您同意分享,请您稍后填写此地址)\n2.服务器中同意公开的房间的房间号(新建的房间默认不公开)\n其中,第1点中内容将会从本程序发送至公开网站服务器,第2点中内容可以被所有人获取\n额外提示:连接到公开网站服务器的IP地址不会被储存\n\n若您不同意,所有信息都不会上传,本程序不会与公开网站通讯\n\n温馨提示:\n若您同意,公开网站每30秒会向本程序发送一个请求,此时本程序会上传上述内容中的第2点至公开网站服务器\n若公开网站无法获取到上述内容(如本程序已关闭/域名无法访问/您撤回了公开分享服务器的授权等情况),则公开网站的服务器会完全删除本程序上传的所有内容,且公开网站服务器不做任何保留,公开网站也会同步移除这些内容\n\n您可以随时撤回该操作\n\n若您同意,请输入yes并换行,输入其他任意内容将视为不同意:\n");
		
		let mut pmps_file = File::create(pmps_file_name)?;
		let mut input = String::new();
		io::stdout().flush()?;
		io::stdin().read_line(&mut input)?;
		let input = input.trim();
		if input.clone() == "yes" {
			println!("\n\n您已同意公开分享此服务器,请继续填写一个安全的可以访问的服务器地址\n\n温馨提示:您必须填写一个安全的地址,即此地址不会泄露您的服务器的原IP地址.\n此地址同时会被保存至程序运行目录下的pmps.txt文件中\n由于自行泄露了服务器原IP地址或对填写的地址不加保护(即您输入的地址可能会暴露服务器原IP地址),导致的任何问题自负\n同时,公开网站服务器也不会接收任何原IP地址(如8.8.8.8)\n公开网站对您填写的服务器地址仅作分享作用,无任何保护作用,您现在填写的服务器地址可以被所有人在公开网站直接查看\n\n若您不同意,您可以删除本程序运行目录下的pmps.txt文件并重新启动本程序以撤回刚才的操作;\n若您同意,请根据以下步骤填写: \n\n1.请输入外部可以访问 {} 端口的地址(如phiramp.example.com:{}),此端口为公开网站服务器访问的端口(不会在公开网站上展示):",pmps_port,pmps_port);
			let mut server_address_1 = String::new();
			io::stdout().flush()?;
			io::stdin().read_line(&mut server_address_1)?;
			let server_address_1 = server_address_1.trim();
			println!("2.请输入外部可以访问 {} 端口的地址(如phiramp.example.com:{}),此端口为游戏服务器的端口,地址即游戏内连接的地址(会在公开网站上展示):\n!注意:这一步填写的服务器地址不会被校验,请注意仔细填写",port,port);
			let mut server_address_2 = String::new();
			io::stdout().flush()?;
			io::stdin().read_line(&mut server_address_2)?;
			let server_address_2 = server_address_2.trim();
			pmps_file.write_all((format!("{}:{}",server_address_1,server_address_2)).as_bytes())?;
		} else {
			pmps_file.write_all(b"false")?;
		}
		println!("\n")
    }
	let pmps = fs::read_to_string(pmps_file_name)?;
	println!("Phira-MP-Server (由BZLZHH修改)");
	println!("端口: {} (游戏端口)\n", port);
	if pmps.clone() == "false" {
		println!("提示: 服务器公开分享功能处于取消状态,若您想开启此功能,请删除本程序运行目录下的pmps.txt文件并重新启动本程序\n");
	} else {
		println!("!提示: 服务器公开分享功能已开启,若您想更改上传的服务器地址或撤回此授权,请删除本程序运行目录下的pmps.txt文件并重新启动本程序\n正在连接到公开网站服务器");
		
		let mut stream = TcpStream::connect("pmpltcp.bzlzhh.top:35345").await?;
		let msg = pmps.clone().to_string().into_bytes();
		stream.write_all(&msg).await?;
		let mut buf = [0; 256];
		let n = stream.read(&mut buf).await?;
		let received_msg = String::from_utf8_lossy(&buf[..n]);
		if received_msg == "ok" {
			println!("连接成功");
			println!("\n提示: 若您在公开网站(pmpl.bzlzhh.top)没有找到您的服务器,请重新启动本程序并等待约30秒.若仍不存在,请删除本程序运行目录下的pmps.txt并重新启动本程序,根据提示重新填写服务器地址");
		} else {
			if received_msg.clone() == "no" {
				println!("连接失败,服务器地址填写错误,请删除本程序运行目录下的pmps.txt并重新启动本程序,根据提示重新填写服务器地址");
			} else {
				println!("连接失败,收到了: {}",received_msg);
			}
		}
	}
    let mut listener: Server = TcpListener::bind(addrs).await?.into();
	listener.sharing_state = Arc::new(pmps.to_string());
	let mut listener_arc = Arc::new(listener);
	
	if pmps.clone() == "false" {
		loop {
			listener_arc.accept();
		}
	}
	else {
		let pmps_addrs: &[SocketAddr] = &[
			SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), pmps_port),
			SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), pmps_port),
		];
		let mut pmps_listener = TcpListener::bind(pmps_addrs).await?;
		loop {
			tokio::select! {
				result = async {
					let result = pmps_listener.accept().await;
					if let Ok((socket, _addr)) = result {
						tokio::spawn(async move {
							process_pmps_connection(socket).await
						});
					}
				} => { }
				result = async{
					listener_arc.accept().await
				} => { }
			}
		}
	}
}

async fn process_pmps_connection(mut socket: TcpStream) -> Result<()> {
	let pmps_file_name = "pmps.txt";
	let prs_file_name = "public_rooms.json";
    let mut buf = [0; 1024];
    let n = match socket.read(&mut buf).await {
        Ok(0) => return Ok(()),
        Ok(n) => n,
        Err(e) => {
            eprintln!("Failed to read from socket: {:?}", e);
            return Ok(());
        }
    };
    let received_msg = String::from_utf8_lossy(&buf[..n]);
    let response = if received_msg.trim() == "GRS" {
        match fs::read_to_string(prs_file_name.clone()) {
            Ok(content) => {
				println!("收到获取公开房间的请求, 返回:{}",content);
				content
			},
            Err(e) => {
                eprintln!("Error reading file: {}", e);
                "[]".to_string()
            }
        }
    } else {
        "".to_string()
    };
    if let Err(e) = socket.write_all(response.as_bytes()).await {
        eprintln!("Failed to write to socket: {:?}", e);
    }
	return Ok(())
}