# frozen_string_literal: true

source('https://rubygems.org')

ruby(file: '.ruby-version')

gem('activesupport')
gem('dotenv')
gem('openssl')
gem('puma')
gem('rackup')
gem('ruby_llm')
gem('ruby_llm-cost', git: 'https://github.com/scott113341/ruby_llm-cost')
gem('sinatra')
gem('twilio-ruby')

group(:development, :test) do
  gem('rspec')
  gem('rubocop')
  gem('vcr')
  gem('webmock')
end
