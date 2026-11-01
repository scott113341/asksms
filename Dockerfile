FROM ruby:4.0.9 AS build

WORKDIR /usr/src/app

COPY Gemfile Gemfile.lock .ruby-version ./
RUN bundle config set --local without development:test && bundle install

FROM ruby:4.0.9-slim

WORKDIR /usr/src/app

COPY --from=build /usr/local/bundle /usr/local/bundle
COPY ./ ./

ENV RACK_ENV=production
ENTRYPOINT ["bundle", "exec", "ruby", "main.rb"]
