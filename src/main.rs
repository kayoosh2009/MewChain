use actix_web::{get, post, web, App, HttpServer, Responder, HttpResponse};
use bip39::{Mnemonic, Language};
use serde::{Serialize, Deserialize};
use sha2::{Sha256, Digest};
use chrono::prelude::*;
use std::sync::Mutex;

// --- СТРУКТУРЫ ДАННЫХ ---

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
        let input = format!("{}{}{}{}{}", self.index, self.timestamp, tx_data, self.previous_hash, self.nonce);
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

// --- ЯДРО БЛОКЧЕЙНА ---

struct Blockchain {
    chain: Vec<Block>,
    pending_transactions: Vec<Transaction>,
}

impl Blockchain {
    fn new() -> Self {
        let genesis_block = Block::new(0, vec![], "0".to_string());
        Blockchain {
            chain: vec![genesis_block],
            pending_transactions: vec![],
        }
    }

    fn add_transaction(&mut self, sender: String, receiver: String, amount: f64) {
        self.pending_transactions.push(Transaction { sender, receiver, amount });
    }

    fn mine_pending_transactions(&mut self) {
        let previous_hash = self.chain.last().unwrap().hash.clone();
        let new_block = Block::new(
            self.chain.len() as u32,
            self.pending_transactions.clone(),
            previous_hash
        );
        self.chain.push(new_block);
        self.pending_transactions = vec![];
    }

    fn get_balance(&self, address: &str) -> f64 {
        let mut balance = 0.0;
        // Начальный капитал для тестов (например, системе)
        if address == "Mew_System" { balance = 1000000.0; }

        for block in &self.chain {
            for tx in &block.transactions {
                if tx.sender == address { balance -= tx.amount; }
                if tx.receiver == address { balance += tx.amount; }
            }
        }
        balance
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
    let mut bc = data.blockchain.lock().unwrap();
    
    let commission = tx.amount * 0.03;
    let total_needed = tx.amount + commission;

    if bc.get_balance(&tx.sender) < total_needed {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Insufficient funds",
            "needed_with_fee": total_needed
        }));
    }

    // Основная транзакция
    bc.add_transaction(tx.sender.clone(), tx.receiver.clone(), tx.amount);
    // Комиссия уходит в казну системы
    bc.add_transaction(tx.sender.clone(), "Mew_Treasury".to_string(), commission);
    
    // Майним блок, чтобы транзакция подтвердилась
    bc.mine_pending_transactions();
    
    HttpResponse::Ok().json(serde_json::json!({
        "status": "Success",
        "fee_paid": commission,
        "new_balance": bc.get_balance(&tx.sender)
    }))
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
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}