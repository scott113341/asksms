# frozen_string_literal: true

require_relative '../app/util'

RSpec.describe Util do
  describe '.split_message' do
    it 'returns the original message when under 160 chars' do
      message = 'Short message'
      result = Util.split_message(message)
      expect(result).to eq([message])
    end

    it 'splits long messages into multiple chunks' do
      long_message = 'a ' * 100
      result = Util.split_message(long_message)
      expect(result).to match([
        start_with('1/2: a'),
        start_with('2/2: a'),
      ])
    end

    it 'preserves word boundaries when splitting' do
      message = 'this message is 300 long ' * 12
      expect(message.length).to eq(300)

      result = Util.split_message(message)
      expect(result).to match([
        start_with('1/2: this message is 300 long'),
        start_with('2/2: message is 300 long this'),
      ])
    end
  end
end
