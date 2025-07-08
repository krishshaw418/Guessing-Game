use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::State,
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::{sink::SinkExt, stream::StreamExt};
use rand::Rng;
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc::UnboundedSender;
use tracing_subscriber;

type Clients = Arc<Mutex<Vec<UnboundedSender<Message>>>>;

#[derive(Clone)]
struct AppState {
    clients: Clients,
    secret_number: Arc<Mutex<u32>>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let state = AppState {
        clients: Arc::new(Mutex::new(Vec::new())), 
        secret_number: Arc::new(Mutex::new(rand::thread_rng().gen_range(1..=100))),
    };

    let app = Router::new()
        .route("/", get(ws_handler))
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    println!("Server listening on ws://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    // create a channel to send messages to this client
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    // add this client's sender to the list
    {
        let mut clients = state.clients.lock().unwrap();
        clients.push(tx.clone()); 
    }

    // task to forward messages from the server to the client
    let write_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    // read messages from client
    while let Some(Ok(msg)) = receiver.next().await {
        if let Message::Text(text) = msg {
            if let Ok(guess) = text.trim().parse::<u32>() {
                let secret = *state.secret_number.lock().unwrap();

                let response = if guess < secret {
                    format!("guessed {} → Too low!", guess)
                } else if guess > secret {
                    format!("guessed {} → Too high!", guess)
                } else {
                    let win_msg = format!("guessed {} → You Win!", guess);
                    // reset secret number
                    *state.secret_number.lock().unwrap() =
                        rand::thread_rng().gen_range(1..=100);
                    win_msg
                };

                broadcast(&state, Message::Text(response)).await;
            } else {
                let _ = tx.send(Message::Text("Invalid input".into()));
            }
        }
    }

    // remove client on disconnect
    {
        let mut clients = state.clients.lock().unwrap();
        clients.retain(|c| !c.same_channel(&tx)); 
    }

    write_task.abort();
}

async fn broadcast(state: &AppState, msg: Message) {
    let clients = state.clients.lock().unwrap();
    for client in clients.iter() {
        let _ = client.send(msg.clone());
    }
}
