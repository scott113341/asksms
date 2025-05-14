# frozen_string_literal: true

require 'active_support/all'
require 'dotenv/load'
require 'ruby_llm'
require 'twilio-ruby'

require_relative 'app/app'

Thread.new do
  loop do
    puts(GC.stat.to_json)
    sleep(1.minute)
  end
end

# Configure RubyLLM
RubyLLM.configure do |config|
  config.openai_api_key = ENV.fetch('OPENAI_API_KEY')
  config.default_model = ENV.fetch('OPENAI_MODEL', 'gpt-4.1-nano')
end

# Initialize Twilio client
TWILIO_CLIENT = Twilio::REST::Client.new(
  ENV.fetch('TWILIO_ACCOUNT_SID'),
  ENV.fetch('TWILIO_AUTH_TOKEN')
)

# Start the application
AskSMS.run!
