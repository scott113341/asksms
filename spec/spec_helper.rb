# frozen_string_literal: true

require 'rspec'
require 'vcr'
require 'webmock/rspec'

Dotenv.load('.example.env')

require_relative '../main'

RSpec.configure do |config|
  config.example_status_persistence_file_path = nil
end

VCR.configure do |c|
  c.hook_into(:webmock)
  c.cassette_library_dir = File.expand_path('cassettes', __dir__)
  c.configure_rspec_metadata!

  %w[
    OPENROUTER_API_KEY
    TWILIO_ACCOUNT_SID
    TWILIO_AUTH_TOKEN
    TWILIO_NUMBER
  ].each do |env_key|
    c.filter_sensitive_data("<#{env_key}>") { ENV[env_key] } if ENV[env_key]
  end

  c.default_cassette_options = {
    record: :new_episodes,
    match_requests_on: %i[method uri body],
    drop_unused_requests: true,
  }
end
