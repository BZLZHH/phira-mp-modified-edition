use crate::{Chart, Record, User};
use anyhow::{bail, Result};
use phira_mp_common::{ClientRoomState, Message, RoomId, RoomState, ServerCommand};
use rand::{seq::SliceRandom, thread_rng};
use std::{
    collections::{HashMap, HashSet},
    ops::Deref,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Weak,
    },
	time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::RwLock;
use tracing::{debug, info};

use std::fs::File;
use std::io::Write;
use crate::ServerState;

const ROOM_MAX_USERS: usize = 100;

#[derive(Clone)]
pub struct UserPlayedResult {
    id: i32,
    user_name: String,
    score: i32,
    accuracy: f32,
    fc_state: bool,
	finish_time: u64,
}
type UsersPlayedResult = Vec<UserPlayedResult>;

#[derive(Clone)]
pub struct OperationPanel {
    pointer_position: u8,
	interface: u8,
	users: Vec<Arc<User>>,
}

#[derive(Clone)]
pub struct ActionState(pub bool,pub String);

#[derive(Default, Debug)]
pub enum InternalRoomState {
    #[default]
    SelectChart,
    WaitForReady {
		start_time: u64,
        started: HashSet<i32>,
    },
    Playing {
        results: HashMap<i32, Record>,
        aborted: HashSet<i32>,
		start_time: u64
    },
}

impl InternalRoomState {
    pub fn to_client(&self, chart: Option<i32>) -> RoomState {
        match self {
            Self::SelectChart => RoomState::SelectChart(chart),
            Self::WaitForReady { .. } => RoomState::WaitingForReady,
            Self::Playing { .. } => RoomState::Playing,
        }
    }
}

pub struct Room {
    pub id: RoomId,
    pub host: RwLock<Weak<User>>,
    pub state: RwLock<InternalRoomState>,

    pub live: AtomicBool,
    pub locked: AtomicBool,
    pub cycle: AtomicBool,
	pub public_enabled: AtomicBool,

    pub users: RwLock<Vec<Weak<User>>>,
    pub users_waiting: RwLock<Vec<Weak<User>>>,
    monitors: RwLock<Vec<Weak<User>>>,
    pub chart: RwLock<Option<Chart>>,
	pub users_played_result: RwLock<UsersPlayedResult>,
	pub panel: RwLock<OperationPanel>,
	
	pub host_time: RwLock<u64>,
}

impl Room {
    pub fn new(id: RoomId, host: Weak<User>) -> Self {
        Self {
            id,
            host: host.clone().into(),
            state: RwLock::default(),

            live: AtomicBool::new(false),
            locked: AtomicBool::new(false),
            cycle: AtomicBool::new(true),
            public_enabled: AtomicBool::new(false),

            users: vec![host].into(),
            users_waiting: Vec::new().into(),
            monitors: Vec::new().into(),
            chart: RwLock::default(),
			users_played_result: Vec::new().into(),
			panel: RwLock::new(OperationPanel{
				pointer_position: 0,
				interface: 0,
				users: Vec::new(),
			}),
			
			host_time: 0.into(),
        }
    }
	
	pub async fn refresh_public_rooms(&self, server_state: Arc<ServerState>) {
		let mut public_rooms_id: Vec<String> = Vec::new();
		let rooms = server_state.rooms.read().await;
		for (room_id, room) in rooms.iter() {
			if room.is_public_enabled() {
				public_rooms_id.push(room_id.to_string());
			}
		}
		let public_rooms_id_string = format!(
			"[{}]",
			public_rooms_id.iter()
				.map(|s| format!(r#""{}""#, s))
				.collect::<Vec<String>>()
				.join(", ")
		);
		let prs_file_name = "public_rooms.json";
		let mut prs_file = File::create(prs_file_name);
		prs_file.expect("REASON").write_all(public_rooms_id_string.as_bytes());
	}

	pub async fn refresh_panel(&self) -> ActionState {
		let panel = self.panel().await;
		let mut msg: String = "".to_string();
		let mut operations_view: String = "".to_string();
		
		match panel.interface {
			0 => {
				let operations = ["开始游戏", "锁定房间", "循环模式", "踢出玩家", "转移房主", "公开房间"];
				operations_view = operations
					.iter()
					.enumerate()
					.map(|(idx, operation)| {
						if idx == panel.pointer_position.clone() as usize {
							format!("->{}", operation)
						} else {
							format!("    {}", operation)
						}
					})
					.collect::<Vec<String>>()
					.join("\n");
			},
			
			1 => {
				let users = self.users().await;
				let mut write_guard = &mut self.panel.write().await;
				write_guard.users = users.clone();
				drop(write_guard);
				let mut operations: Vec<String> = users.iter().map(|user| user.name.clone()).collect();
				operations.insert(0, "返回".to_string());
				operations_view = operations
						.iter()
						.enumerate()
						.map(|(idx, operation)| {
							if idx == panel.pointer_position.clone() as usize {
								format!("->{}", operation)
							} else {
								format!("    {}", operation)
							}
						})
						.collect::<Vec<String>>()
						.join("\n");
				
			},
			
			2_u8..=u8::MAX => todo!(),
		}
		msg = format!("\n房主面板:\n{}",operations_view);
		self.send_as_server_to_host(msg.clone()).await;
		return ActionState(true, msg.clone());
	}
	
	pub async fn panel_pointer_ok(&self,user: Arc<User>) -> ActionState {
		{
			let panel = self.panel().await;
			match panel.interface.clone() {
				0 => {
					//["开始游戏", "锁定房间", "循环模式", "踢出玩家", "转移房主", "公开房间"];
					match panel.pointer_position.clone() {
						0 => {
							return ActionState(true, "".to_string()); // true->start game; false->return msg
						}
						
						1 => {
							{
								if self.id.to_string() != "public" {
									let lock = !self.is_locked();
									info!(
										user = user.id.clone(),
										room = self.id.to_string(),
										lock,
										"lock room"
									);
									self.locked.store(lock, Ordering::SeqCst);
									self.send(Message::LockRoom { lock }).await;
								}
								else {
									return ActionState(false, "公共房间不允许锁定房间".to_string());
								}
							}
							let is_cycle: String = if self.is_locked() { "".to_string() } else { "取消".to_string() };
							return ActionState(false, format!("已{}锁定\n(这不是错误消息)", is_cycle));
						}
						
						2 => {
							{
								if self.id.to_string() != "public" {
									let cycle = !self.is_cycle();
									info!(
										user = user.id.clone(),
										room = self.id.to_string(),
										cycle,
										"cycle room"
									);
									self.cycle.store(cycle, Ordering::SeqCst);
									self.send(Message::CycleRoom { cycle }).await;
								}
								else {
									return ActionState(false, "公共房间不允许修改模式".to_string());
								}
							}
							let mode: String = if self.is_cycle() { "循环".to_string() } else { "普通".to_string() };
							return ActionState(false, format!("已切换至{}模式\n(这不是错误消息)", mode));
						}
						
						3 => {
							{
								let mut write_guard = &mut self.panel.write().await;
								write_guard.interface = 1;
								write_guard.pointer_position = 0;
								drop(write_guard);
							}
							self.refresh_panel().await;
							return ActionState(false, "已操作\n(这不是错误消息)".to_string());
						}
						
						4 => {
							debug!(room = self.id.to_string(), "cycling");
							let host = Weak::clone(&*self.host.read().await);
							let new_host = {
								let users = self.users().await;
								let index = users
									.iter()
									.position(|it| host.ptr_eq(&Arc::downgrade(it)))
									.map(|it| (it + 1) % users.len())
									.unwrap_or_default();
								users.into_iter().nth(index).unwrap()
							};
							*self.host.write().await = Arc::downgrade(&new_host);
							self.reset_host_time(None).await;
							self.send(Message::NewHost { user: new_host.id }).await;
							if let Some(old) = host.upgrade() {
								old.try_send(ServerCommand::ChangeHost(false)).await;
								self.send_as_server(format!("原房主 {} 已将房主转移至 {}",old.name.clone(),new_host.name.clone())).await;
							}
							new_host.try_send(ServerCommand::ChangeHost(true)).await;
							
		
							let msg_to_new_host = format!("\n[重要] 房主面板(开始游戏=确定;锁定房间=↑;循环模式=↓):\n");
							self.send_as_server_to_host(msg_to_new_host.clone()).await;
							return ActionState(false, format!("已转移至 {}\n(这不是错误消息)", new_host.name.clone()));
						}
						
						5 => {
							{
								if self.id.to_string() != "public" {
									let public_enable = !self.is_public_enabled();
									self.public_enabled.store(public_enable, Ordering::SeqCst);
								}
								else {
									return ActionState(false, "公共房间不允许修改房间公开状态".to_string());
								}
							}
							let public_state: String = if self.is_public_enabled() { "公开此房间.\n所有人都可以在公开网站(pmpl.bzlzhh.top)上查看此房间(若此服务器支持)".to_string() } else { "取消公开此房间.".to_string() };
							self.send_as_server(format!("房主已{}",public_state.clone())).await;
							self.refresh_public_rooms(user.server.clone()).await;
							
							return ActionState(false, format!("已{}\n(这不是错误消息)", public_state.clone()));
						}
						6_u8..=u8::MAX => todo!(),
					}
				}
				
				1 => {
					if panel.pointer_position == 0 {
						{
							let mut write_guard = &mut self.panel.write().await;
							write_guard.interface = 0;
							write_guard.pointer_position = 0;
							drop(write_guard);
						}
						self.refresh_panel().await;
						return ActionState(false, "已操作\n(这不是错误消息)".to_string());
					}
					let users_panel = panel.users;
					let users_now = self.users().await;
					let user_kicked = users_panel[panel.pointer_position as usize - 1].clone();
					if !users_now.contains(&user_kicked) {
						return ActionState(false, format!("{} 已离开", user_kicked.name.clone()));
					}
					self.send_as_server(format!("{} 已被房主 {} 踢出", user_kicked.name.clone(), user.name.clone())).await;
					self.on_user_leave(&*user_kicked.clone()).await;
					{
						let mut write_guard = &mut self.panel.write().await;
						write_guard.pointer_position = 0;
						drop(write_guard);
					}
					self.refresh_panel().await;
					return ActionState(false, format!("已踢出 {}\n(这不是错误消息)", user_kicked.name.clone()));
				}
				
				2_u8..=u8::MAX => todo!(),
			}
			let mut write_guard = &mut self.panel.write().await;
			write_guard.pointer_position = panel.pointer_position.clone() + 1;
			drop(write_guard);
		}
		return ActionState(false, "意外错误".to_string())
		
	}

	pub async fn panel_pointer_down(&self) -> ActionState {
		{
			let panel = self.panel().await;
			let pointer_position_max: u8 = match panel.interface.clone() {
				0 => 5,
				1 => self.users().await.len().clone() as u8,
				2_u8..=u8::MAX => todo!(),
			};
			if panel.pointer_position.clone() + 1 > pointer_position_max {
				return ActionState(false,"".to_string())
			}
			let mut write_guard = &mut self.panel.write().await;
			write_guard.pointer_position = panel.pointer_position.clone() + 1;
			drop(write_guard);
		}
		let result: ActionState = self.refresh_panel().await;
		return result;
	}
	
	pub async fn panel_pointer_up(&self) -> ActionState {
		{
			let panel = self.panel().await;
			if panel.pointer_position.clone() <= 0 {
				return ActionState(false,"".to_string())
			}
			let mut write_guard = &mut self.panel.write().await;
			write_guard.pointer_position = panel.pointer_position.clone() - 1;
			drop(write_guard);
		}
		let result: ActionState = self.refresh_panel().await;
		return result;
	}

    pub fn is_live(&self) -> bool {
        self.live.load(Ordering::SeqCst)
    }

    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::SeqCst)
    }

    pub fn is_cycle(&self) -> bool {
        self.cycle.load(Ordering::SeqCst)
    }
	
    pub fn is_public_enabled(&self) -> bool {
        self.public_enabled.load(Ordering::SeqCst)
    }
	
	pub async fn clear_waiting_users(&self) {
        *self.users_waiting.write().await = Vec::new();
	}

    pub async fn client_room_state(&self) -> RoomState {
        self.state
            .read()
            .await
            .to_client(self.chart.read().await.as_ref().map(|it| it.id))
    }

	pub async fn get_users_played_result(&self) -> UsersPlayedResult {
		let users_played_result = self.users_played_result.read().await;
		users_played_result.clone() 
	}

    pub async fn client_state(&self, user: &User) -> ClientRoomState {
        ClientRoomState {
            id: self.id.clone(),
            state: self.client_room_state().await,
            live: self.is_live(),
            locked: self.is_locked(),
            cycle: self.is_cycle(),
            is_host: self.check_host(user).await.is_ok(),
            is_ready: matches!(&*self.state.read().await, InternalRoomState::WaitForReady { started, .. } if started.contains(&user.id)),
            users: self
                .users
                .read()
                .await
                .iter()
                .chain(self.monitors.read().await.iter())
                .filter_map(|it| it.upgrade().map(|it| (it.id, it.to_info())))
                .collect(),
        }
    }

    pub async fn on_state_change(&self) {
        self.broadcast(ServerCommand::ChangeState(self.client_room_state().await))
            .await;
    }
	
    pub async fn add_user(&self, mut user: Weak<User>, monitor: bool, is_user_waiting: bool) -> bool {
		let users = self.users().await;
		if users.is_empty() {
			let new_user=user.clone();
            *self.host.write().await = new_user.clone();
			self.reset_host_time(None).await;
			if let Some(arc_user) = new_user.upgrade() {
				/*
				let user_ref: &User = &*arc_user;
				self.send(Message::NewHost { user: user_ref.id }).await;
				user_ref.try_send(ServerCommand::ChangeHost(true)).await;
				*/
			}
		}
        
		if is_user_waiting {
			let mut guard = self.users.write().await;
			let mut guard_waiting = self.users_waiting.write().await;
            guard.retain(|it| it.strong_count() > 0);
            guard_waiting.retain(|it| it.strong_count() > 0);
            if guard.len() + guard_waiting.len() >= ROOM_MAX_USERS {
                false
            } else {
                guard_waiting.push(user.clone());
                guard.push(user.clone());
                true
            }
		} else if monitor {
            let mut guard = self.monitors.write().await;
            guard.retain(|it| it.strong_count() > 0);
            guard.push(user);
            true
        } 
		else {
            let mut guard = self.users.write().await;
            guard.retain(|it| it.strong_count() > 0);
            if guard.len() >= ROOM_MAX_USERS {
                false
            } else {
                guard.push(user);
                true
            }
        }
    }
	
	pub async fn reset_host_time(&self, time: Option<u64>) {
		let mut time_: u64 = 0;
		if time == None {
			let current_time = SystemTime::now();
			let timestamp = current_time.duration_since(UNIX_EPOCH).expect("Time went backwards");
			time_ = timestamp.as_secs();
		} else {
			time_ = time.unwrap().clone();
		}
		*self.host_time.write().await = time_;
	}
	
	pub async fn check_host_time_proper(&self) {
		let guard = self.host_time.read().await;
		let host_time = (*guard).clone();
		drop(guard);
		
		let current_time = SystemTime::now();
		let timestamp = current_time.duration_since(UNIX_EPOCH).expect("Time went backwards");
		let now_time = timestamp.as_secs();
		
		if host_time != 0 && now_time - host_time >= 360
		{
			let host_arc = self.host().await;
			let host: &User = &*host_arc;
			self.send_as_server(format!("房主 {} 选择谱面超时,已被踢出",(*host).name.clone())).await;
			self.on_user_leave(host).await;
		}
	}
	
	pub async fn check_ready_time_proper(&self) {
		let guard = self.state.read().await;
		let mut start_time_: u64 = 0;
		let mut started_users: HashSet<i32> = HashSet::new();
		if let InternalRoomState::WaitForReady { start_time, started } = &*guard {
			start_time_ = *start_time;
			started_users = started.clone();
		};
		drop(guard);
		let current_time = SystemTime::now();
		let timestamp = current_time.duration_since(UNIX_EPOCH).expect("Time went backwards");
		let now_time = timestamp.as_secs();
		
		if now_time - start_time_ >= 90 {
			let mut users=self.users().await;
			let users_waiting=self.users_waiting().await;
			users.retain(|user| !users_waiting.iter().any(|waiting_user| waiting_user.id == user.id));
			users.retain(|user| !started_users.iter().any(|started_users| *started_users == user.id));
			let mut sb_users: String = "".to_string();
			for user in users {
				self.on_user_leave(&user).await;
				sb_users = format!("{}、{}",sb_users,user.name.clone());
			}
			self.send_as_server(format!("以下玩家未准备且已超时,被踢出: {}",sb_users)).await;
		}
	}
	
	pub async fn check_game_time_proper(&self) -> bool {
		let mut users_played_result = self.get_users_played_result().await;
			
		let current_time = SystemTime::now();
		let timestamp = current_time.duration_since(UNIX_EPOCH).expect("Time went backwards");
		let now_time = timestamp.as_secs();
		
		if users_played_result.len().clone() < 1 {
			let guard = self.state.read().await;
			let mut start_time_: u64 = 0; 
			if let InternalRoomState::Playing { start_time, .. } = *guard {
				start_time_ = start_time.clone();
			}
			drop(guard);
			if now_time - start_time_ >= 480 { //8min
				let mut users=self.users().await;
				let users_waiting=self.users_waiting().await;
				users.retain(|user| !users_waiting.iter().any(|waiting_user| waiting_user.id == user.id));
				
				let mut sb_users: String = "? ".to_string();
				for user in users {
					self.on_user_leave(&user).await;
					sb_users = format!("{}、{}",sb_users,user.name.clone());
				}
				self.send_as_server(format!("(以下玩家游玩超时,已被踢出: {})",sb_users)).await;
				false
			} else {
				true
			}
			
		} else {
			let first_finish_time = users_played_result[0].finish_time;
			if now_time - first_finish_time >= 120 {
				let mut users=self.users().await;
				let users_waiting=self.users_waiting().await;
				users.retain(|user| !users_waiting.iter().any(|waiting_user| waiting_user.id == user.id));
				users.retain(|user| !users_played_result.iter().any(|result| result.id == user.id));

				let mut sb_users: String = "? ".to_string();
				for user in users {
					self.on_user_leave(&user).await;	
					sb_users = format!("{}、{}",sb_users,user.name.clone());
				}
				self.send_as_server(format!("(以下玩家游玩超时,已被踢出: {})",sb_users)).await;
				false
			} else {
				true
			}
		}
	}
	
	pub async fn panel(&self) -> OperationPanel {
		let panel = self.panel.read().await;
		panel.clone()
    }
	
	pub async fn host(&self) -> Arc<User> {
        self.host
            .read()
            .await
            .upgrade()
			.unwrap()
    }

    pub async fn users(&self) -> Vec<Arc<User>> {
        self.users
            .read()
            .await
            .iter()
            .filter_map(|it| it.upgrade())
            .collect()
    }
	
	pub async fn users_waiting(&self) -> Vec<Arc<User>> {
        self.users_waiting
            .read()
            .await
            .iter()
            .filter_map(|it| it.upgrade())
            .collect()
    }

    pub async fn monitors(&self) -> Vec<Arc<User>> {
        self.monitors
            .read()
            .await
            .iter()
            .filter_map(|it| it.upgrade())
            .collect()
    }

    pub async fn check_host(&self, user: &User) -> Result<()> {
        if self.host.read().await.upgrade().map(|it| it.id) != Some(user.id) {
            bail!("只有房主才能进行此操作");
        }
        Ok(())
    }

    #[inline]
    pub async fn send(&self, msg: Message) {
        self.broadcast(ServerCommand::Message(msg)).await;
    }

    pub async fn broadcast(&self, cmd: ServerCommand) {
        debug!("broadcast {cmd:?}");
        for session in self
            .users()
            .await
            .into_iter()
            .chain(self.monitors().await.into_iter())
        {
            session.try_send(cmd.clone()).await;
        }
    }

    pub async fn broadcast_monitors(&self, cmd: ServerCommand) {
        for session in self.monitors().await {
            session.try_send(cmd.clone()).await;
        }
    }
	
	pub async fn add_score(&self, id: i32, user_name: String, score: i32, accuracy: f32, full_combo: bool) {
		let current_time = SystemTime::now();
		let timestamp = current_time.duration_since(UNIX_EPOCH).expect("Time went backwards");
		let now_time = timestamp.as_secs();
		let mut dictionary: UserPlayedResult = UserPlayedResult {
			id,
			user_name,
			score,
			accuracy,
			fc_state: full_combo,
			finish_time: now_time,
		};
		let mut write_lock = self.users_played_result.write().await;
		write_lock.push(dictionary);
		drop(write_lock);
	}
	
	#[inline]
	pub async fn send_as_server(&self ,content: String) {
		self.send(Message::Chat {
            user: 195677,
            content,
        })
        .await;
	}
	
	#[inline]
	pub async fn send_as_server_to_host(&self ,content: String) {
		&self.host.read().await.upgrade().expect("REASON").try_send(ServerCommand::Message(Message::Chat {
            user: 195677,
            content,
        }))
        .await;
	}

    #[inline]
    pub async fn send_as(&self, user: &User, content: String) {
        self.send(Message::Chat {
            user: user.id,
            content,
        })
        .await;
    }

    /// Return: should the room be dropped
    #[must_use]
    pub async fn on_user_leave(&self, user: &User) -> bool {
        self.send(Message::LeaveRoom {
            user: user.id,
            name: user.name.clone(),
        })
        .await;
        *user.room.write().await = None;
        (if user.monitor.load(Ordering::SeqCst) {
            &self.monitors
        } else {
            &self.users
        })
        .write()
        .await
        .retain(|it| it.upgrade().map_or(false, |it| it.id != user.id));
		let id_string = &self.id.to_string().clone();
        if self.check_host(user).await.is_ok() {
            info!("host disconnected!");
            let users = self.users().await;
            let users_waiting = self.users_waiting().await;
            if users.is_empty() && users_waiting.is_empty() {
                info!("room users all disconnected, dropping room");
                return true;
            }
			else if !(users.is_empty() && users_waiting.is_empty()) {
                let user = users.choose(&mut thread_rng()).unwrap();
                debug!("selected {} as host", user.id);
                *self.host.write().await = Arc::downgrade(user);
				self.reset_host_time(None).await;
                self.send(Message::NewHost { user: user.id }).await;
				let msg_to_new_host = format!("\n[重要] 房主面板(开始游戏=确定;锁定房间=↑;循环模式=↓):\n");
				self.send_as_server_to_host(msg_to_new_host.clone()).await;
                user.try_send(ServerCommand::ChangeHost(true)).await;
            }
        }
		self.check_all_ready().await;
        false
    }

    pub async fn reset_game_time(&self) {
        for user in self.users().await {
            user.game_time
                .store(f32::NEG_INFINITY.to_bits(), Ordering::SeqCst);
        }
    }

    pub async fn check_all_ready(&self) {
        let guard = self.state.read().await;
        match guard.deref() {
            InternalRoomState::WaitForReady { started, .. } => {
                let mut users=self.users().await;
				let users_waiting=self.users_waiting().await;
				users.retain(|user| !users_waiting.iter().any(|waiting_user| waiting_user.id == user.id));
                if users.clone()
                    .into_iter()
                    .chain(self.monitors().await.into_iter())
                    .all(|it| started.contains(&it.id))
                {
                    drop(guard);
                    info!(room = self.id.to_string(), "game start");
                    self.send(Message::StartPlaying).await;
					self.reset_host_time(Some(0)).await;
                    self.reset_game_time().await;
					*self.users_played_result.write().await = Vec::new(); 
					let current_time = SystemTime::now();
					let timestamp = current_time.duration_since(UNIX_EPOCH).expect("Time went backwards");
					let seconds = timestamp.as_secs();
                    *self.state.write().await = InternalRoomState::Playing {
                        results: HashMap::new(),
                        aborted: HashSet::new(),
						start_time: seconds,
                    };
                    self.on_state_change().await;
                }
            }
            InternalRoomState::Playing { results, aborted, .. } => {
				let mut users=self.users().await;
				let users_waiting=self.users_waiting().await;
				users.retain(|user| !users_waiting.iter().any(|waiting_user| waiting_user.id == user.id));
                if users.clone()
                    .into_iter()
                    .all(|it| results.contains_key(&it.id) || aborted.contains(&it.id))
                {
                    drop(guard);
                    self.send(Message::GameEnd).await;
					let mut users_played_result = self.get_users_played_result().await;
					users_played_result.sort_by_key(|result| std::cmp::Reverse(result.score));
					self.send_as_server("游戏结束, 结果如下:".to_string()).await;
					let mut is_server_joined: bool = false;
					let mut i = 1;
					for mut result in users_played_result.clone() {
						let user_name: String = result.user_name.clone();
						let user_id = result.id.clone();
						let score: String = format!("{:07}", result.score);
						let grade = if result.score == 1000000 {
							"φ"
						} else if result.score >= 960000 && result.score < 1000000 {
							"V"
						} else if result.score >= 920000 && result.score < 960000 {
							"S"
						} else if result.score >= 880000 && result.score < 920000 {
							"A"
						} else if result.score >= 820000 && result.score < 880000 {
							"B"
						} else if result.score >= 700000 && result.score < 820000 {
							"C"
						} else if result.score < 700000 {
							"F"
						} else {
							"纪"
						};
						let acc: String = format!("{}%", format!("{:.2}", result.accuracy * 100.0));
						let mut fc_state: String = "".to_string();
						if result.fc_state {
							fc_state = "Full Combo".to_string();
						}
						let msg: String = format!("第{}  玩家:{}  等级:{}  分数:{}  ACC:{}  {}",i,user_name,grade,score,acc,fc_state);
						self.send_as_server(msg).await;
						i += 1;
					}
					if users_played_result.len().clone() < 1 {
						self.send_as_server("没有结果吗(⊙_⊙)？".to_string()).await;
					}
					self.clear_waiting_users().await;
                    *self.state.write().await = InternalRoomState::SelectChart;
                    if self.is_cycle() {
                        debug!(room = self.id.to_string(), "cycling");
                        let host = Weak::clone(&*self.host.read().await);
                        let new_host = {
                            let users = self.users().await;
                            let index = users
                                .iter()
                                .position(|it| host.ptr_eq(&Arc::downgrade(it)))
                                .map(|it| (it + 1) % users.len())
                                .unwrap_or_default();
                            users.into_iter().nth(index).unwrap()
                        };
                        *self.host.write().await = Arc::downgrade(&new_host);
						self.reset_host_time(None).await;
                        self.send(Message::NewHost { user: new_host.id }).await;
                        if let Some(old) = host.upgrade() {
                            old.try_send(ServerCommand::ChangeHost(false)).await;
                        }
                        new_host.try_send(ServerCommand::ChangeHost(true)).await;
						let msg_to_new_host = format!("\n[重要] 房主面板(开始游戏=确定;锁定房间=↑;循环模式=↓):\n");
						self.send_as_server_to_host(msg_to_new_host.clone()).await;
                    }
                    self.on_state_change().await;
                }
            }
            _ => {}
        }
    }
}
