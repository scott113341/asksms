# frozen_string_literal: true

require 'rack'
require 'rspec'
require 'twilio-ruby'

require_relative '../app/app'
require_relative '../app/util'

RSpec.describe AskSMS do
  let(:app) { AskSMS.new }
  let(:request) { Rack::MockRequest.new(app) }

  describe 'GET /' do
    it 'responds' do
      res = request.get('/')
      expect(res.status).to eq(200)
      expect(res.body).to eq('askSMS is running')
    end
  end
end
