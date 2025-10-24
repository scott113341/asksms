# frozen_string_literal: true

require "rack"
require "rspec"
require "twilio-ruby"

RSpec.describe AskSMS do
  let(:app) { AskSMS.new }
  let(:request) { Rack::MockRequest.new(app) }

  describe "GET /" do
    it "responds" do
      res = request.get("/")
      expect(res.status).to eq(200)
      expect(res.body).to eq("askSMS is running")
    end
  end

  describe "POST /sms" do
    it "responds", :vcr do
      allow(Util).to receive(:twilio_valid_request?).and_return(true)

      expect(Util).to receive(:send_messages) do |z|
        expect(z[:to]).to eq("+11231231234")
        expect(z[:messages]).to be_an(Array).and(have_attributes(length: 3))
        z[:messages].each { |m| expect(m.length).to be <= 160 }
      end

      res = request.post(
        "/sms",
        params: {
          Body: "Who is the current president?",
          From: "+11231231234"
        }
      )
      expect(res.status).to eq(200)
      expect(res.content_type).to eq("text/xml;charset=utf-8")
      expect(res.body).to eq(Util.empty_twiml_response)
    end
  end
end
