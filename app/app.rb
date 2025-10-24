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
    halt(500) unless Util.twilio_valid_request?(request)

    params = request.POST
    incoming_message = params['Body']
    from_number = params['From']

    chat, answer = Util.get_answer(incoming_message)
    messages = Util.split_into_messages(answer)

    puts("Cost: $#{chat.total_cost}")
    pp(messages)

    # Send response(s)
    Util.send_messages(
      to: from_number,
      messages: messages,
    )

    # Return empty TwiML response
    content_type('text/xml')
    Util.empty_twiml_response
  end
end
