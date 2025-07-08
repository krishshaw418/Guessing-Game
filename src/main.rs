use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::State,
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::{sink::SinkExt, stream::StreamExt};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc::UnboundedSender;
use tracing_subscriber;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct GameMessage {
    player_id: String,
    player_name: String,
    message: String,
    round: u32,
}

#[derive(Clone, Debug)]
struct Player {
    id: String,
    name: String,
    sender: UnboundedSender<Message>,
    wins: u32,
    active: bool, // Track if player is currently connected
}

#[derive(Clone, Debug)]
struct GameState {
    current_round: u32,
    secret_number: u32,
    players: HashMap<String, Player>,
    active_connections: HashMap<String, String>, // player_id -> player_name for active connections
    round_active: bool,
    max_rounds: u32,
    max_players: usize,
    game_started: bool,
}

impl GameState {
    fn new() -> Self {
        Self {
            current_round: 1,
            secret_number: rand::thread_rng().gen_range(1..=100),
            players: HashMap::new(),
            active_connections: HashMap::new(),
            round_active: true,
            max_rounds: 5,
            max_players: 4,
            game_started: false,
        }
    }

    fn reset_for_next_round(&mut self) {
        self.current_round += 1;
        self.secret_number = rand::thread_rng().gen_range(1..=100);
        self.round_active = true;
        // Keep connections alive - don't clear active_connections or mark players inactive
        
        println!("Round {} started. Secret number: {}", self.current_round, self.secret_number);
    }

    fn is_game_over(&self) -> bool {
        self.current_round > self.max_rounds
    }

    fn can_accept_new_connection(&self) -> bool {
        // Only allow new players to join in round 1
        self.active_connections.len() < self.max_players && 
        self.current_round == 1 && 
        !self.is_game_over() && 
        self.round_active
    }

    fn get_leaderboard(&self) -> Vec<(String, u32)> {
        let mut leaderboard: Vec<(String, u32)> = self.players
            .values()
            .map(|p| (p.name.clone(), p.wins))
            .collect();
        leaderboard.sort_by(|a, b| b.1.cmp(&a.1));
        leaderboard
    }

    fn add_or_reconnect_player(&mut self, player_id: String, player_name: String, sender: UnboundedSender<Message>) -> bool {
        // Check if this is a returning player
        if let Some(existing_player) = self.players.get_mut(&player_id) {
            // Reconnecting player
            existing_player.sender = sender;
            existing_player.active = true;
            existing_player.name = player_name.clone(); // Update name in case it changed
            self.active_connections.insert(player_id, player_name);
            return true;
        }

        // Check if we can accept a new player
        if self.active_connections.len() >= self.max_players {
            return false;
        }

        // New player
        let player = Player {
            id: player_id.clone(),
            name: player_name.clone(),
            sender,
            wins: 0,
            active: true,
        };
        self.players.insert(player_id.clone(), player);
        self.active_connections.insert(player_id, player_name);
        true
    }

    fn remove_active_connection(&mut self, player_id: &str) {
        self.active_connections.remove(player_id);
        if let Some(player) = self.players.get_mut(player_id) {
            player.active = false;
        }
    }

    fn get_active_player_count(&self) -> usize {
        self.active_connections.len()
    }
}

#[derive(Clone)]
struct AppState {
    game_state: Arc<Mutex<GameState>>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let state = AppState {
        game_state: Arc::new(Mutex::new(GameState::new())),
    };

    let app = Router::new()
        .route("/", get(ws_handler))
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    println!("Server listening on ws://{}", addr);
    println!("Game Rules:");
    println!("- Maximum 4 players per round");
    println!("- 5 rounds total");
    println!("- Guess the number between 1-100");
    println!("- First to guess correctly wins the round");
    println!("- Player with most wins across all rounds is the champion!");
    
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
    let player_id = Uuid::new_v4().to_string();
    let (mut sender, mut receiver) = socket.split();

    // Check if we can accept this player
    let can_join = {
        let game_state = state.game_state.lock().unwrap();
        game_state.can_accept_new_connection()
    };

    if !can_join {
        let rejection_msg = {
            let game_state = state.game_state.lock().unwrap();
            if game_state.is_game_over() {
                "Game has ended! Final results have been announced. Please wait for a new game to start.".to_string()
            } else if game_state.current_round > 1 {
                format!("Game is already in progress (Round {}). Please wait for the next game to start.", 
                       game_state.current_round)
            } else if game_state.active_connections.len() >= game_state.max_players {
                format!("Round {} is full! Maximum {} players allowed. Please wait for the next game.", 
                       game_state.current_round, game_state.max_players)
            } else if !game_state.round_active {
                "Round is ending. Please wait for the next round to start.".to_string()
            } else {
                "Cannot join at this time. Please try again later.".to_string()
            }
        };
        
        let _ = sender.send(Message::Text(rejection_msg)).await;
        let _ = sender.close().await;
        return;
    }

    // Create channel for this player
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    // Send welcome message and ask for player name
    let welcome_msg = format!("Welcome! You are Player {}. Please enter your name:", player_id[..8].to_uppercase());
    let _ = tx.send(Message::Text(welcome_msg));

    // Task to forward messages from server to client
    let write_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    let mut player_name = String::new();
    let mut name_set = false;

    // Read messages from client
    while let Some(Ok(msg)) = receiver.next().await {
        if let Message::Text(text) = msg {
            let input = text.trim();
            
            if !name_set {
                // First message should be the player name
                player_name = input.to_string();
                name_set = true;
                
                // Try to add player to game state
                let added = {
                    let mut game_state = state.game_state.lock().unwrap();
                    game_state.add_or_reconnect_player(player_id.clone(), player_name.clone(), tx.clone())
                };

                if !added {
                    let _ = tx.send(Message::Text("Sorry, the round is now full. Please wait for the next round.".to_string()));
                    break;
                }
                
                // Send game info
                let game_info = {
                    let game_state = state.game_state.lock().unwrap();
                    format!("Welcome {}! Round {}/{} - Guess a number between 1-100. Players: {}/{}", 
                           player_name, game_state.current_round, game_state.max_rounds, 
                           game_state.get_active_player_count(), game_state.max_players)
                };
                let _ = tx.send(Message::Text(game_info));
                
                // Show current leaderboard if not first round
                {
                    let game_state = state.game_state.lock().unwrap();
                    if game_state.current_round > 1 {
                        let leaderboard = game_state.get_leaderboard();
                        if !leaderboard.is_empty() {
                            let mut leaderboard_msg = "Current Standings:\n".to_string();
                            for (i, (name, wins)) in leaderboard.iter().enumerate() {
                                leaderboard_msg.push_str(&format!("{}. {} - {} wins\n", i + 1, name, wins));
                            }
                            let _ = tx.send(Message::Text(leaderboard_msg));
                        }
                    }
                }
                
                // Broadcast player joined
                broadcast_to_others(&state, &player_id, &format!("{} joined the game!", player_name)).await;
                continue;
            }

            // Check if round is active
            let round_active = {
                let game_state = state.game_state.lock().unwrap();
                game_state.round_active
            };

            if !round_active {
                let _ = tx.send(Message::Text("Round is over! Waiting for next round...".to_string()));
                continue;
            }

            // Process guess
            if let Ok(guess) = input.parse::<u32>() {
                if guess < 1 || guess > 100 {
                    let _ = tx.send(Message::Text("Please guess a number between 1 and 100".to_string()));
                    continue;
                }

                let (response, round_won, current_round, is_game_over) = {
                    let mut game_state = state.game_state.lock().unwrap();
                    let secret = game_state.secret_number;
                    let current_round = game_state.current_round;

                    let response = if guess < secret {
                        format!("{} guessed {} → Too low! (Round {})", player_name, guess, current_round)
                    } else if guess > secret {
                        format!("{} guessed {} → Too high! (Round {})", player_name, guess, current_round)
                    } else {
                        // Correct guess - player wins the round
                        game_state.round_active = false;
                        if let Some(player) = game_state.players.get_mut(&player_id) {
                            player.wins += 1;
                        }
                        format!("🎉 {} guessed {} → WINS ROUND {}! 🎉", player_name, guess, current_round)
                    };

                    let round_won = guess == secret;
                    let is_game_over = current_round >= game_state.max_rounds;
                    (response, round_won, current_round, is_game_over)
                };

                // Broadcast the result
                broadcast_to_all(&state, &response).await;

                if round_won {
                    // Wait a moment before ending round
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    
                    // Check if game is over
                    if is_game_over {
                        // Game is completely over - end game and disconnect all
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        end_game(&state).await;
                    } else {
                        // Round ended but game continues - start next round
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        end_round_and_start_next(&state).await;
                    }
                    
                    // Only break if the entire game is over
                    if is_game_over {
                        break;
                    }
                }
            } else {
                let _ = tx.send(Message::Text("Invalid input. Please enter a number between 1-100".to_string()));
            }
        }
    }

    // Remove player connection on disconnect
    {
        let mut game_state = state.game_state.lock().unwrap();
        game_state.remove_active_connection(&player_id);
    }

    if name_set {
        broadcast_to_others(&state, &player_id, &format!("{} left the game", player_name)).await;
    }

    write_task.abort();
}

async fn broadcast_to_all(state: &AppState, message: &str) {
    let active_players = {
        let game_state = state.game_state.lock().unwrap();
        game_state.active_connections.keys().cloned().collect::<Vec<_>>()
    };

    let players = {
        let game_state = state.game_state.lock().unwrap();
        game_state.players.clone()
    };

    for player_id in active_players {
        if let Some(player) = players.get(&player_id) {
            let _ = player.sender.send(Message::Text(message.to_string()));
        }
    }
}

async fn broadcast_to_others(state: &AppState, sender_id: &str, message: &str) {
    let active_players = {
        let game_state = state.game_state.lock().unwrap();
        game_state.active_connections.keys().cloned().collect::<Vec<_>>()
    };

    let players = {
        let game_state = state.game_state.lock().unwrap();
        game_state.players.clone()
    };

    for player_id in active_players {
        if player_id != sender_id {
            if let Some(player) = players.get(&player_id) {
                let _ = player.sender.send(Message::Text(message.to_string()));
            }
        }
    }
}

async fn end_round_and_start_next(state: &AppState) {
    let (current_round, leaderboard) = {
        let game_state = state.game_state.lock().unwrap();
        (game_state.current_round, game_state.get_leaderboard())
    };

    // Send round end message and leaderboard
    let mut leaderboard_msg = format!("=== ROUND {} ENDED ===\n", current_round);
    leaderboard_msg.push_str("Current Standings:\n");
    
    for (i, (name, wins)) in leaderboard.iter().enumerate() {
        leaderboard_msg.push_str(&format!("{}. {} - {} wins\n", i + 1, name, wins));
    }
    
    leaderboard_msg.push_str("\nGet ready for the next round!");

    broadcast_to_all(state, &leaderboard_msg).await;

    // Wait a moment, then start next round
    tokio::time::sleep(Duration::from_secs(3)).await;
    
    // Start next round (keeps all connections alive)
    start_next_round(state).await;
}

async fn end_round_and_disconnect_all(state: &AppState) {
    let (current_round, leaderboard) = {
        let game_state = state.game_state.lock().unwrap();
        (game_state.current_round, game_state.get_leaderboard())
    };

    // Send round end message and leaderboard
    let mut leaderboard_msg = format!("=== ROUND {} ENDED ===\n", current_round);
    leaderboard_msg.push_str("Current Standings:\n");
    
    for (i, (name, wins)) in leaderboard.iter().enumerate() {
        leaderboard_msg.push_str(&format!("{}. {} - {} wins\n", i + 1, name, wins));
    }
    
    if current_round < 5 {
        leaderboard_msg.push_str("\nConnection will close. Reconnect for next round!");
    }

    broadcast_to_all(state, &leaderboard_msg).await;

    // Wait for message delivery
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Close all active connections
    let active_players = {
        let game_state = state.game_state.lock().unwrap();
        game_state.active_connections.keys().cloned().collect::<Vec<_>>()
    };

    let players = {
        let game_state = state.game_state.lock().unwrap();
        game_state.players.clone()
    };

    for player_id in active_players {
        if let Some(player) = players.get(&player_id) {
            let _ = player.sender.send(Message::Close(None));
        }
    }
}

async fn start_next_round(state: &AppState) {
    {
        let mut game_state = state.game_state.lock().unwrap();
        game_state.reset_for_next_round();
    }

    let current_round = {
        let game_state = state.game_state.lock().unwrap();
        game_state.current_round
    };

    // Notify all connected players that the new round has started
    let start_msg = format!("🎮 ROUND {} STARTED! 🎮\nGuess a number between 1-100!", current_round);
    broadcast_to_all(state, &start_msg).await;

    println!("Round {} started with existing players!", current_round);
}

async fn end_game(state: &AppState) {
    let leaderboard = {
        let game_state = state.game_state.lock().unwrap();
        game_state.get_leaderboard()
    };

    let mut final_msg = "🏆 GAME OVER - FINAL RESULTS 🏆\n".to_string();
    
    if !leaderboard.is_empty() {
        final_msg.push_str("Final Standings:\n");
        for (i, (name, wins)) in leaderboard.iter().enumerate() {
            let medal = match i {
                0 => "🥇",
                1 => "🥈", 
                2 => "🥉",
                _ => "  ",
            };
            final_msg.push_str(&format!("{} {}. {} - {} wins\n", medal, i + 1, name, wins));
        }
        
        if let Some((winner, wins)) = leaderboard.first() {
            final_msg.push_str(&format!("\n🎊 Congratulations {}! You are the champion with {} wins! 🎊", winner, wins));
        }
    }

    final_msg.push_str("\nThank you for playing! Connection will close shortly.");
    
    broadcast_to_all(state, &final_msg).await;
    
    // Wait for message delivery
    tokio::time::sleep(Duration::from_secs(5)).await;
    
    // Close all connections
    let active_players = {
        let game_state = state.game_state.lock().unwrap();
        game_state.active_connections.keys().cloned().collect::<Vec<_>>()
    };

    let players = {
        let game_state = state.game_state.lock().unwrap();
        game_state.players.clone()
    };

    for player_id in active_players {
        if let Some(player) = players.get(&player_id) {
            let _ = player.sender.send(Message::Close(None));
        }
    }
    
    println!("Game completed! Final results sent to all players.");
    
    // Reset game state for a new game
    {
        let mut game_state = state.game_state.lock().unwrap();
        *game_state = GameState::new();
    }
    
    println!("Game reset. Ready for new players!");
}