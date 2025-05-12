require 'sinatra/base'

require_relative './util.rb'

class AskSMS < Sinatra::Base
  set(:server, 'webrick')
  set(:host_authorization, { permitted_hosts: [] })

  get '/' do
    "askSMS is running"
  end

  post '/sms' do
    # Validate request is from Twilio
    validator = Twilio::Security::RequestValidator.new(ENV['TWILIO_AUTH_TOKEN'])
    url = request.url
    params = request.POST
    signature = request.env['HTTP_X_TWILIO_SIGNATURE']
    halt(500) unless validator.validate(url, params, signature)
    
    incoming_message = params['Body']
    from_number = params['From']
    
    # Get response from ChatGPT
    chat = RubyLLM.chat
    response = chat.with_instructions(
      "Keep answers under about 600 characters and focus on being clear and direct. Do not use emojis, or Markdown, just plain text."
    ).ask(incoming_message)

    pp response
    
    answer = response.content

    if answer.length > 800
      answer = chat.ask("Please make the answer more concise.").content
      pp answer
    end
    
    # Split message if needed
    messages = Util.split_message(answer)
    pp messages
    
    # Send response(s)
    messages.each do |message|
      TWILIO_CLIENT.messages.create(
        from: ENV['TWILIO_PHONE_NUMBER'],
        to: from_number,
        body: message,
        smart_encoded: true
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
