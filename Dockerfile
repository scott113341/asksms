FROM ruby:3.4.8

WORKDIR /usr/src/app

COPY Gemfile Gemfile.lock .ruby-version ./
RUN bundle install
COPY ./ ./

ENV RACK_ENV=production
ENTRYPOINT ["bundle", "exec", "ruby", "main.rb"]
