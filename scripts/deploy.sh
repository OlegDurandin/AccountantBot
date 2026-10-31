#!/bin/sh
set -eu

app_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$app_dir"

if [ ! -f .env ]; then
    printf 'ERROR: не найден %s/.env\n' "$app_dir" >&2
    exit 1
fi

compose() {
    docker compose --env-file .env "$@"
}

printf 'Собираю новую версию; текущий бот продолжает работать...\n'
compose build accountant-bot

printf 'Предупреждаю беседы о переключении...\n'
if ! compose run --rm --no-deps accountant-bot accountant-bot notify-updating; then
    printf 'WARNING: уведомление перед обновлением отправлено не во все беседы.\n' >&2
fi

printf 'Переключаю контейнер на новую версию...\n'
compose up -d --no-build accountant-bot

attempt=1
ready=false
while [ "$attempt" -le 30 ]; do
    running=$(docker inspect --format '{{.State.Running}}' accountant-bot 2>/dev/null || true)
    if [ "$running" = "true" ] \
        && docker logs --tail=100 accountant-bot 2>&1 | grep -q 'connected to VK Long Poll'; then
        ready=true
        break
    fi
    sleep 1
    attempt=$((attempt + 1))
done

if [ "$ready" != "true" ]; then
    printf 'ERROR: контейнер accountant-bot не подключился к VK Long Poll.\n' >&2
    compose logs --tail=100 accountant-bot >&2
    exit 1
fi

printf 'Сообщаю беседам о завершении обновления...\n'
if ! compose run --rm --no-deps accountant-bot accountant-bot notify-updated; then
    printf 'WARNING: уведомление после обновления отправлено не во все беседы.\n' >&2
fi

compose ps accountant-bot
printf 'Обновление завершено.\n'
