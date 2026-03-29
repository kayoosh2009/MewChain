use actix_web::{get, post, web, App, HttpResponse, HttpServer, Responder};
use bip39::{Language, Mnemonic};
use chrono::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Mutex;
use std::collections::HashMap;

// --- СТРУКТУРЫ ДАННЫХ ---

const TG_BOT_TOKEN: &str = "ТВОЙ_ТОКЕН_БОТА";
const TG_CHAT_ID: &str = "@ТВОЙ_КАНАЛ_ИЛИ_ID";

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
async fn send_to_telegram(message: String) {
    let url = format!("https://api.telegram.org/bot{}/sendMessage", TG_BOT_TOKEN);
    let client = reqwest::Client::new();
    let res = client
        .post(url)
        .json(&serde_json::json!({
            "chat_id": TG_CHAT_ID,
            "text": message,
            "parse_mode": "Markdown"
        }))
        .send()
        .await;

    match res {
        Ok(_) => println!("✅ Блок отправлен в Telegram"),
        Err(e) => eprintln!("❌ Ошибка отправки в ТГ: {}", e),
    }
}

// --- ЯДРО БЛОКЧЕЙНА ---
struct Blockchain {
    chain: Vec<Block>,
    pending_transactions: Vec<Transaction>,
    last_rewards: HashMap<String, i64>,
}

impl Blockchain {
    fn new() -> Self {
        let genesis_block = Block::new(0, vec![], "0".to_string());
        Blockchain {
            chain: vec![genesis_block],
            pending_transactions: vec![],
            last_rewards: HashMap::new(),
        }
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
        // Начальный капитал для тестов (например, системе)
        if address == "Mew_System" {
            balance = 1000000.0;
        }

        for block in &self.chain {
            for tx in &block.transactions {
                if tx.sender == address {
                    balance -= tx.amount;
                }
                if tx.receiver == address {
                    balance += tx.amount;
                }
            }
        }
        balance
    }

    async fn claim_daily_reward(&mut self, address: String) -> Result<String, String> {
        let now = Utc::now().timestamp();
        let day_in_seconds = 86400; // 24 часа в секундах

        // Проверяем по нашей "базе" HashMap
        if let Some(last_time) = self.last_rewards.get(&address) {
            if now - last_time < day_in_seconds {
                let wait_time = day_in_seconds - (now - last_time);
                return Err(format!("Рано! Приходи через {} сек.", wait_time));
            }
        }

        // Обновляем время и даем монеты
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

        self.chain.push(new_block.clone());
        self.pending_transactions = vec![];

        // Отправляем отчет в Telegram
        let tg_msg = format!(
            "📦 *Новый Блок #{}*\n Hash: `{}`\n Transactions: {}\n Nonce: {}",
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
    let mut entropy = [0u8; 16]; // 16 байт (128 бит) для создания 12 слов
    rand::RngCore::fill_bytes(&mut rng, &mut entropy);

    // Создаем мнемонику из случайных байт
    let mnemonic = Mnemonic::from_entropy_in(Language::English, &entropy).unwrap();

    HttpResponse::Ok().json(serde_json::json!({
        "seed_phrase": mnemonic.to_string(),
        "warning": "Никому не показывайте эти 12 слов!"
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

#[get("/reward/{address}")]
async fn get_daily_reward(data: web::Data<AppState>, path: web::Path<String>) -> impl Responder {
    let address = path.into_inner();
    let mut bc = data.blockchain.lock().unwrap(); //

    match bc.claim_daily_reward(address).await {
        Ok(msg) => HttpResponse::Ok().json(serde_json::json!({"status": "Success", "message": msg})),
        Err(err) => HttpResponse::BadRequest().json(serde_json::json!({"status": "Error", "message": err})),
    }
}

// Просмотр всей цепочки (для отладки)
#[get("/chain")]
async fn get_chain(data: web::Data<AppState>) -> impl Responder {
    let bc = data.blockchain.lock().unwrap();
    HttpResponse::Ok().json(&bc.chain)
}

// --- ЗАПУСК ---

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let app_state = web::Data::new(AppState {
        blockchain: Mutex::new(Blockchain::new()),
    });

    println!("🐾 MewChain Node запущен!");
    println!("URL: http://127.0.0.1:8080");

    HttpServer::new(move || {
        App::new()
            .app_data(app_state.clone())
            .service(create_wallet)
            .service(get_balance_api)
            .service(send_coins_api)
            .service(get_chain)
            .service(get_daily_reward)
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}
