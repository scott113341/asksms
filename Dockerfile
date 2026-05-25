FROM ruby:4.0.5

WORKDIR /usr/src/app

COPY Gemfile Gemfile.lock .ruby-version ./
RUN bundle install
COPY ./ ./

ENV RACK_ENV=production
ENTRYPOINT ["bundle", "exec", "ruby", "main.rb"]
