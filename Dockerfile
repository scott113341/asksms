FROM ruby:3.4.3

WORKDIR /usr/src/app

COPY Gemfile Gemfile.lock ./
RUN bundle install
COPY ./ ./

ENV RACK_ENV=production
ENTRYPOINT ["ruby", "main.rb"]
