# frozen_string_literal: true

require 'active_support/all'
require 'dotenv/load'
require 'ruby_llm'
require 'twilio-ruby'

require_relative 'app/app'

# Configure RubyLLM
RubyLLM.configure do |config|
  config.openrouter_api_key = ENV.fetch('OPENROUTER_API_KEY')
  config.default_model = ENV.fetch('OPENROUTER_MODEL', 'openai/gpt-5-nano')
end

# Initialize Twilio client
TWILIO_CLIENT = Twilio::REST::Client.new(
  ENV.fetch('TWILIO_ACCOUNT_SID'),
  ENV.fetch('TWILIO_AUTH_TOKEN')
)

# Start the application
AskSMS.run! if __FILE__ == $PROGRAM_NAME
