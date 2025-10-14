# frozen_string_literal: true

require 'sinatra/base'

require_relative 'util'

class AskSMS < Sinatra::Base
  set(:host_authorization, { permitted_hosts: [] })

  get('/') do
    'askSMS is running'
  end

  post('/sms') do
    # Validate request is from Twilio
    validator = Twilio::Security::RequestValidator.new(ENV.fetch('TWILIO_AUTH_TOKEN', nil))
    url = request.url
    params = request.POST
    signature = request.env['HTTP_X_TWILIO_SIGNATURE']
    halt(500) unless validator.validate(url, params, signature)

    incoming_message = params['Body']
    from_number = params['From']

    chat = RubyLLM
      .chat
      .with_params(plugins: [{ id: 'web' }])
      .with_instructions(
        'Keep answers under about 600 characters and focus on being clear and direct. Do not use ' \
        'emojis or Markdown, just plain text compatible with GSM-7 encoding.',
      )

    response = chat.ask(incoming_message)

    pp(response)

    if response.content.length > 800
      response = chat.ask('Please make your answer more concise.')
      pp(response)
    end

    # Split message if needed
    answer = response.content
    messages = Util.split_message(answer)
    pp(messages)

    # Send response(s)
    messages.each do |message|
      TWILIO_CLIENT.messages.create(
        from: ENV.fetch('TWILIO_NUMBER'),
        to: from_number,
        body: message,
        smart_encoded: true,
      )
    end

    # Return empty TwiML response
    content_type('text/xml')
    <<~XML
      <?xml version="1.0" encoding="UTF-8"?>
      <Response></Response>
    XML
  end
end
