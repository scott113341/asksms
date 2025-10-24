# frozen_string_literal: true

RSpec.describe Util do
  describe '::get_answer' do
    it 'gets an answer from the LLM', :vcr do
      chat, answer = Util.get_answer('What time does MainStage Brewing in Lyons open on Saturday?')
      expect(chat).to be_a(RubyLLM::Chat)
      expect(answer).to match('opens at 11:00 AM on Saturday')
    end
  end

  describe '::split_into_messages' do
    it 'returns the original message when under 160 chars' do
      message = 'Short message'
      result = Util.split_into_messages(message)
      expect(result).to eq([message])
    end

    it 'splits long messages into multiple chunks' do
      long_message = 'a ' * 100
      result = Util.split_into_messages(long_message)
      expect(result).to match([
        start_with('1/2: a'),
        start_with('2/2: a'),
      ])
    end

    it 'preserves word boundaries when splitting' do
      message = 'this message is 300 long ' * 12
      expect(message.length).to eq(300)

      result = Util.split_into_messages(message)
      expect(result).to match([
        start_with('1/2: this message is 300 long'),
        start_with('2/2: message is 300 long this'),
      ])
    end
  end
end
