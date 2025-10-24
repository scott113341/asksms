# frozen_string_literal: true

module Util
  # Validates that an incoming request is from Twilio using the X-Twilio-Signature header.
  # Returns true when valid, false otherwise.
  def self.twilio_valid_request?(request, auth_token = ENV.fetch('TWILIO_AUTH_TOKEN', nil))
    return false if auth_token.nil? || auth_token.empty?

    validator = Twilio::Security::RequestValidator.new(auth_token)
    url = request.url
    params = request.POST
    signature = request.env['HTTP_X_TWILIO_SIGNATURE']
    validator.validate(url, params, signature)
  end

  # Asks the model to answer the incoming message. If the first response is too long,
  # ask the model to make it more concise. Returns the final text answer and the chat object
  # so the caller may access cost and other details.
  def self.get_answer(incoming_message)
    chat = RubyLLM
      .chat
      .with_params(plugins: [{ id: 'web' }], max_tokens: 128)
      .with_instructions(
        'Focus on being clear and direct. DO NOT use emojis, Markdown, or citations. Use only ' \
        'raw plaintext compatible with GSM-7 encoding.',
      )

    response = chat.ask(incoming_message)
    pp(response)

    if response.content.length > 800
      puts("Response too long (#{response.content.length}), asking for more concise answer")
      response = chat.ask('Please make your answer more concise.')
      pp(response)
    end

    [chat, response.content]
  end

  # Sends an array of messages via the provided Twilio client.
  def self.send_messages(to:, messages:, from: ENV.fetch('TWILIO_NUMBER'), client: TWILIO_CLIENT)
    messages.each do |message|
      client.messages.create(
        from: from,
        to: to,
        body: message,
        smart_encoded: true,
      )
    end
  end

  # Builds the empty TwiML response body returned to Twilio webhooks.
  def self.empty_twiml_response
    <<~XML
      <?xml version="1.0" encoding="UTF-8"?>
      <Response></Response>
    XML
  end

  def self.split_into_messages(message, max_length = 160)
    return [message] if message.length <= max_length

    chunks = []
    current_chunk = ''
    words = message.split(' ')

    words.each do |word|
      if (current_chunk + ' ' + word).length <= max_length - 5 # Reserve space for "1/2: "
        current_chunk += ' ' + word
      else
        chunks << current_chunk.strip
        current_chunk = word
      end
    end

    chunks << current_chunk.strip if current_chunk.length > 0

    chunks.map.with_index(1) do |chunk, index|
      "#{index}/#{chunks.length}: #{chunk}"
    end
  end
end
