#!/bin/bash

# Path to the bot directory
BOT_DIR="/Users/leohermoso/RustroverProjects/whirlpool_bot"
# Log file to track restarts
LOG_FILE="$BOT_DIR/bot_monitor.log"

# Your bot command with all necessary parameters
# Modify this line with your actual command line arguments
BOT_CMD="cargo run -- --pool-address Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE --interval 120 --invest 0.15"

# Function to log messages
log_message() {
  echo "$(date '+%Y-%m-%d %H:%M:%S') - $1" >> "$LOG_FILE"
  echo "$(date '+%Y-%m-%d %H:%M:%S') - $1"
}

# Change to the bot directory
cd "$BOT_DIR" || {
  log_message "ERROR: Could not change to bot directory $BOT_DIR"
  exit 1
}

# Check if bot is running
if pgrep -f "whirlpool_bot.*--pool-address" > /dev/null; then
  log_message "Whirlpool bot is already running"
else
  log_message "Whirlpool bot is not running. Starting it now..."
  
  # Start the bot and run in background
  nohup $BOT_CMD >> "$BOT_DIR/bot.log" 2>&1 &
  
  # Check if start was successful
  sleep 5
  if pgrep -f "whirlpool_bot.*--pool-address" > /dev/null; then
    log_message "Successfully started Whirlpool bot"
  else
    log_message "ERROR: Failed to start Whirlpool bot"
  fi
fi 