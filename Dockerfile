FROM ruby:4.0.9

WORKDIR /usr/src/app

COPY Gemfile Gemfile.lock .ruby-version ./
RUN bundle install
COPY ./ ./

ENV RACK_ENV=production
ENTRYPOINT ["bundle", "exec", "ruby", "main.rb"]
