FROM scratch AS build

FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc
COPY --from=build /etc/ssl/certs/ /etc/ssl/certs/
COPY --from=build /app/target/release/yearn-vote-notifications /usr/local/bin/yearn-vote-notifications
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/yearn-vote-notifications"]
CMD ["poll-once"]
