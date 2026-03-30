use actix_web::{get, post, web, App, HttpResponse, HttpServer, Responder};
use bip39::{Language, Mnemonic};
use chrono::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Mutex;
use std::collections::HashMap;
use actix_cors::Cors;
use actix_files::Files;
use dotenv::dotenv; 
use lazy_static::lazy_static; 
use rusqlite::{params, Connection};

// --- СТРУКТУРЫ ДАННЫХ ---

lazy_static! {
    // Используем полный путь std::env::var прямо внутри макроса
    static ref TG_BOT_TOKEN: String = std::env::var("TG_BOT_TOKEN").unwrap_or_else(|_| "NOT_SET".to_string());
    static ref TG_CHAT_ID: String = std::env::var("TG_CHAT_ID").unwrap_or_else(|_| "NOT_SET".to_string());
}
const APY: f64 = 0.07; // 7% годовых

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Transaction {
    sender: String,
    receiver: String,
    amount: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Block {
    index: u32,
    timestamp: i64,
    transactions: Vec<Transaction>,
    previous_hash: String,
    hash: String,
    nonce: u64,
}

impl Block {
    fn new(index: u32, transactions: Vec<Transaction>, previous_hash: String) -> Self {
        let mut block = Block {
            index,
            timestamp: Utc::now().timestamp(),
            transactions,
            previous_hash,
            hash: String::new(),
            nonce: 0,
        };
        block.mine(2); // Сложность 2 (00...)
        block
    }

    fn calculate_hash(&self) -> String {
        let mut hasher = Sha256::new();
        let tx_data = serde_json::to_string(&self.transactions).unwrap();
        let input = format!(
            "{}{}{}{}{}",
            self.index, self.timestamp, tx_data, self.previous_hash, self.nonce
        );
        hasher.update(input);
        format!("{:x}", hasher.finalize())
    }

    fn mine(&mut self, difficulty: usize) {
        let target = "0".repeat(difficulty);
        while !self.hash.starts_with(&target) {
            self.nonce += 1;
            self.hash = self.calculate_hash();
        }
    }
}

// --- ОТПРАВКА В ТЕЛЕГРАМ ---
async fn send_to_telegram(text: String) {
    let url = format!("https://api.telegram.org/bot{}/sendMessage", *TG_BOT_TOKEN);
    let client = reqwest::Client::new();
    let _ = client.post(url)
        .json(&serde_json::json!({
            "chat_id": *TG_CHAT_ID,
            "text": text,
            "parse_mode": "Markdown"
        }))
        .send()
        .await;
}

// --- ЯДРО БЛОКЧЕЙНА ---
struct Blockchain {
    chain: Vec<Block>,
    pending_transactions: Vec<Transaction>,
    last_rewards: HashMap<String, i64>,
    hashes_in_current_block: u8,
}

impl Blockchain {
    fn new() -> Self {
        // Сначала создаем таблицы, если их нет
        Self::init_db();

        // Пытаемся загрузить блоки из базы
        let saved_blocks = Self::load_all_blocks();

        let mut bc = Blockchain {
            chain: saved_blocks.clone(), // Загружаем историю
            pending_transactions: vec![],
            last_rewards: HashMap::new(),
            hashes_in_current_block: 0,
        };

        if bc.chain.is_empty() {
            // Если база пуста, создаем Genesis блок
            let genesis = Block::new(0, vec![], "0".to_string());
            bc.save_block_to_db(&genesis);
            bc.chain.push(genesis);
        }

        bc
    }

    fn init_db() {
        let conn = Connection::open("mewchain.db").expect("Failed to open DB");
        conn.execute(
            "CREATE TABLE IF NOT EXISTS blocks (
                idx INTEGER PRIMARY KEY,
                timestamp INTEGER,
                transactions TEXT,
                prev_hash TEXT,
                hash TEXT,
                nonce INTEGER
            )",
            [],
        ).ok();
    }

    fn save_block_to_db(&self, block: &Block) {
        let conn = Connection::open("mewchain.db").unwrap();
        let txs_json = serde_json::to_string(&block.transactions).unwrap();
        
        conn.execute(
            "INSERT INTO blocks (idx, timestamp, transactions, prev_hash, hash, nonce) 
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![block.index, block.timestamp, txs_json, block.previous_hash, block.hash, block.nonce],
        ).ok();
    }

    fn load_all_blocks() -> Vec<Block> {
        let conn = Connection::open("mewchain.db").unwrap();
        let mut stmt = conn.prepare("SELECT idx, timestamp, transactions, prev_hash, hash, nonce FROM blocks ORDER BY idx ASC").unwrap();
        
        let block_iter = stmt.query_map([], |row| {
            let txs_raw: String = row.get(2)?;
            Ok(Block {
                index: row.get(0)?,
                timestamp: row.get(1)?,
                transactions: serde_json::from_str(&txs_raw).unwrap_or_default(),
                previous_hash: row.get(3)?,
                hash: row.get(4)?,
                nonce: row.get(5)?,
            })
        }).unwrap();

        block_iter.map(|b| b.unwrap()).collect()
    }

    fn add_transaction(&mut self, sender: String, receiver: String, amount: f64) {
        self.pending_transactions.push(Transaction {
            sender,
            receiver,
            amount,
        });
    }

    fn get_balance(&self, address: &str) -> f64 {
        let mut balance = 0.0;
        let now = Utc::now().timestamp();
        let seconds_in_year = 31536000.0; 

        if address == "Mew_System" {
            balance = 1000000.0;
        }

        for block in &self.chain {
            let time_diff = now - block.timestamp;
            let time_in_years = time_diff as f64 / seconds_in_year;

            for tx in &block.transactions {
                if tx.sender == address {
                    balance -= tx.amount;
                }
                if tx.receiver == address {
                    let staked_amount = tx.amount * (1.0 + (APY * time_in_years));
                    balance += staked_amount;
                }
            }
        }
        balance
    }

    async fn claim_daily_reward(&mut self, address: String) -> Result<String, String> {
        let now = Utc::now().timestamp();
        let day_in_seconds = 86400; 

        if let Some(last_time) = self.last_rewards.get(&address) {
            if now - last_time < day_in_seconds {
                let wait_time = day_in_seconds - (now - last_time);
                return Err(format!("Рано! Приходи через {} сек.", wait_time));
            }
        }

        self.last_rewards.insert(address.clone(), now);
        self.add_transaction("Mew_System".to_string(), address.clone(), 10.0);
        
        self.mine_pending_transactions().await;
        Ok(format!("10 MEW зачислены на {}", address))
    }

    async fn mine_pending_transactions(&mut self) {
        let previous_hash = self.chain.last().unwrap().hash.clone();
        let new_block = Block::new(
            self.chain.len() as u32,
            self.pending_transactions.clone(),
            previous_hash,
        );

        // ВАЖНО: Сохраняем свежий блок в SQLite
        self.save_block_to_db(&new_block);

        self.chain.push(new_block.clone());
        self.pending_transactions = vec![];

        let tg_msg = format!(
            "📦 *New Block Mined: #{}*\n Hash: `{}`\n Total Txs: {}\n Nonce: {}\n Status: `Confirmed`",
            new_block.index,
            new_block.hash,
            new_block.transactions.len(),
            new_block.nonce
        );
        send_to_telegram(tg_msg).await;
    }
}

// Состояние приложения для API
struct AppState {
    blockchain: Mutex<Blockchain>,
}

// --- API ЭНДПОИНТЫ ---

// Генерация сид-фразы (Страница 2 MewWallet)
#[get("/wallet/create")]
async fn create_wallet() -> impl Responder {
    let mut rng = rand::thread_rng();
    let mut entropy = [0u8; 16]; 
    rand::RngCore::fill_bytes(&mut rng, &mut entropy);

    // Генерируем 12 слов (мнемонику)
    let mnemonic = Mnemonic::from_entropy_in(Language::English, &entropy).unwrap();
    let seed_phrase = mnemonic.to_string();
    
    // Генерируем уникальный адрес MEW_... на основе сид-фразы
    let address = derive_address(&seed_phrase);

    // Формируем лог для Telegram на английском
    let tg_msg = format!(
        "🆕 *New Wallet Registered*\n\n\
        📍 Address: `{}`\n\
        📡 Network: `MewChain Mainnet`\n\
        🛡 Status: `Secured`",
        address
    );
    
    // Отправляем уведомление в канал
    send_to_telegram(tg_msg).await;

    // Возвращаем JSON пользователю
    HttpResponse::Ok().json(serde_json::json!({
        "seed_phrase": seed_phrase,
        "address": address,
        "symbol": "MEW",
        "network": "Mainnet",
        "warning": "CRITICAL: Never share your seed phrase with anyone!"
    }))
}

// Проверка баланса (Страница 4 MewWallet)
#[get("/balance/{address}")]
async fn get_balance_api(data: web::Data<AppState>, path: web::Path<String>) -> impl Responder {
    let address = path.into_inner();
    let bc = data.blockchain.lock().unwrap();
    let balance = bc.get_balance(&address);

    HttpResponse::Ok().json(serde_json::json!({
        "address": address,
        "balance": balance,
        "symbol": "MEW"
    }))
}

// Отправка монет с комиссией 3% (Страница 5 MewWallet)
#[post("/send")]
async fn send_coins_api(data: web::Data<AppState>, tx: web::Json<Transaction>) -> impl Responder {
    {
        let mut bc = data.blockchain.lock().unwrap();
        let commission = tx.amount * 0.03;
        let total_needed = tx.amount + commission;

        if bc.get_balance(&tx.sender) < total_needed {
            return HttpResponse::BadRequest().json(serde_json::json!({
                "error": "Insufficient funds",
                "needed_with_fee": total_needed
            }));
        }

        bc.add_transaction(tx.sender.clone(), tx.receiver.clone(), tx.amount);
        bc.add_transaction(tx.sender.clone(), "Mew_Treasury".to_string(), commission);
    }

    {
        let mut bc = data.blockchain.lock().unwrap();
        bc.mine_pending_transactions().await;
    }

    HttpResponse::Ok().json(serde_json::json!({"status": "Success"}))
}

// Просмотр всей цепочки (для отладки)
#[get("/chain")]
async fn get_chain(data: web::Data<AppState>) -> impl Responder {
    let bc = data.blockchain.lock().unwrap();
    HttpResponse::Ok().json(&bc.chain)
}

#[post("/mine/submit")]
async fn submit_nonce(data: web::Data<AppState>, info: web::Json<serde_json::Value>) -> impl Responder {
    let mut bc = data.blockchain.lock().unwrap();
    
    let address = info["address"].as_str().unwrap_or("Unknown").to_string();

    // 1. Reward for ACTIVE hash
    bc.add_transaction("Mew_System".to_string(), address.clone(), 0.0012);
    bc.hashes_in_current_block += 1;

    // Send notification to TG for EVERY hash as you requested
    let hash_msg = format!(
        "⛏ *Hash Accepted*\nWorker: `{}`\nReward: `0.0012 MEW`\nBlock Progress: `{}/25`",
        address, bc.hashes_in_current_block
    );
    send_to_telegram(hash_msg).await;

    // 2. Check if we reached 25 hashes to close the block
    if bc.hashes_in_current_block >= 25 {
        bc.hashes_in_current_block = 0; // Reset counter
        
        // Finalize the block
        bc.mine_pending_transactions().await;

        return HttpResponse::Ok().json(serde_json::json!({
            "status": "Success",
            "message": "Block completed. 25/25 hashes collected."
        }));
    }

    HttpResponse::Ok().json(serde_json::json!({
        "status": "Accepted",
        "hash_reward": 0.0012,
        "progress": format!("{}/25", bc.hashes_in_current_block)
    }))
}

// --- ЗАПУСК ---

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenv().ok(); 

    // Получаем порт от системы или используем 8080 как запасной
    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse::<u16>()
        .expect("PORT must be a number");

    let app_state = web::Data::new(AppState {
        blockchain: Mutex::new(Blockchain::new()),
    });

    println!("🐾 MewWallet Server starting on 0.0.0.0:{}", port);

    HttpServer::new(move || {
        let cors = Cors::permissive(); 

        App::new()
            .wrap(cors)
            .app_data(app_state.clone())
            .service(create_wallet)
            .service(get_balance_api)
            .service(send_coins_api)
            .service(get_chain)
            .service(get_daily_reward)
            .service(submit_nonce)
            .service(Files::new("/", "./static").index_file("index.html"))
    })
    .bind(("0.0.0.0", port))? // Привязываемся к динамическому порту
    .run()
    .await
}
