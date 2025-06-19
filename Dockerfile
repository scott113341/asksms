FROM ruby:3.4.4

WORKDIR /usr/src/app

COPY Gemfile Gemfile.lock .ruby-version ./
RUN bundle install
COPY ./ ./

ENV RACK_ENV=production
ENTRYPOINT ["ruby", "main.rb"]
