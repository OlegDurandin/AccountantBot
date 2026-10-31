#!/bin/sh

# Проверяет развёрнутый AccountantBot по SSH.
# Коды возврата: 0 — всё хорошо, 1 — есть предупреждения, 2 — критическая ошибка.

set -u

script_dir=$(CDPATH= cd -P "$(dirname "$0")" && pwd)
project_dir=$(dirname "$script_dir")
env_file="${CHECK_ENV_FILE:-$project_dir/.env}"

read_env_value() {
    [ -f "$env_file" ] || return 0
    awk -F= -v wanted="$1" '
        $1 == wanted {
            sub(/^[^=]*=/, "")
            sub(/\r$/, "")
            print
            exit
        }
    ' "$env_file"
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
    cat <<'EOF'
Использование:
  ./scripts/check.sh [SSH_TARGET] [APP_DIR]

Примеры:
  ./scripts/check.sh
  ./scripts/check.sh user@server.example.com /opt/accountant-bot

Переменные окружения:
  VPS_SSH_TARGET               SSH-адрес из .env (обязателен)
  VPS_APP_DIR                  каталог приложения из .env (обязателен)
  CHECK_ENV_FILE               другой путь к .env
  CHECK_SSH_TARGET             временно переопределить SSH-адрес
  CHECK_APP_DIR                временно переопределить каталог приложения
  CHECK_CONTAINER              имя контейнера (по умолчанию accountant-bot)
  CHECK_LOG_SINCE              период проверки логов (по умолчанию 30m)
  CHECK_BACKUP_MAX_AGE_HOURS   допустимый возраст бэкапа (по умолчанию 30)
  CHECK_CONNECT_TIMEOUT        таймаут SSH в секундах (по умолчанию 10)
  CHECK_COLOR                  auto, always или never (по умолчанию auto)

Коды возврата:
  0 — OK, 1 — WARNING, 2 — CRITICAL
EOF
    exit 0
fi

env_target=$(read_env_value VPS_SSH_TARGET)
env_app_dir=$(read_env_value VPS_APP_DIR)
target="${1:-${CHECK_SSH_TARGET:-$env_target}}"
app_dir="${2:-${CHECK_APP_DIR:-$env_app_dir}}"
container="${CHECK_CONTAINER:-accountant-bot}"
log_since="${CHECK_LOG_SINCE:-30m}"
backup_max_age_hours="${CHECK_BACKUP_MAX_AGE_HOURS:-30}"
connect_timeout="${CHECK_CONNECT_TIMEOUT:-10}"
color_mode="${CHECK_COLOR:-auto}"

case "$app_dir" in
    '') printf 'CRITICAL: задайте VPS_APP_DIR в %s\n' "$env_file" >&2; exit 2 ;;
    /*) ;;
    *) printf 'CRITICAL: APP_DIR должен быть абсолютным путём\n' >&2; exit 2 ;;
esac
case "$target" in
    '') printf 'CRITICAL: задайте VPS_SSH_TARGET в %s\n' "$env_file" >&2; exit 2 ;;
    -*|*[!A-Za-z0-9_.@:-]*) printf 'CRITICAL: некорректный VPS_SSH_TARGET\n' >&2; exit 2 ;;
esac
case "$app_dir" in
    *[!A-Za-z0-9_./-]*) printf 'CRITICAL: APP_DIR содержит недопустимые символы\n' >&2; exit 2 ;;
esac
case "$container" in
    ''|*[!A-Za-z0-9_.-]*) printf 'CRITICAL: некорректное имя контейнера\n' >&2; exit 2 ;;
esac
case "$log_since" in
    ''|*[!0-9smhd]*) printf 'CRITICAL: некорректный CHECK_LOG_SINCE\n' >&2; exit 2 ;;
esac
case "$backup_max_age_hours" in
    ''|*[!0-9]*) printf 'CRITICAL: CHECK_BACKUP_MAX_AGE_HOURS должен быть числом\n' >&2; exit 2 ;;
esac
case "$connect_timeout" in
    ''|*[!0-9]*) printf 'CRITICAL: CHECK_CONNECT_TIMEOUT должен быть числом\n' >&2; exit 2 ;;
esac
case "$color_mode" in
    auto)
        if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
            color_enabled=1
        else
            color_enabled=0
        fi
        ;;
    always) color_enabled=1 ;;
    never) color_enabled=0 ;;
    *) printf 'CRITICAL: CHECK_COLOR должен быть auto, always или never\n' >&2; exit 2 ;;
esac

printf 'Проверка AccountantBot на %s...\n' "$target"

ssh -o BatchMode=yes -o "ConnectTimeout=$connect_timeout" "$target" \
    sh -s -- "$app_dir" "$container" "$log_since" "$backup_max_age_hours" "$color_enabled" <<'REMOTE_CHECK'
app_dir=$1
container=$2
log_since=$3
backup_max_age_hours=$4
color_enabled=$5

if [ "$color_enabled" = '1' ]; then
    escape=$(printf '\033')
    green="${escape}[1;32m"
    orange="${escape}[38;5;208m"
    red="${escape}[1;31m"
    cyan="${escape}[1;36m"
    bold="${escape}[1m"
    reset="${escape}[0m"
else
    green=''
    orange=''
    red=''
    cyan=''
    bold=''
    reset=''
fi

warnings=0
criticals=0
checks=0

ok() {
    checks=$((checks + 1))
    printf '%s✓%s %s\n' "$green" "$reset" "$1"
}

warn() {
    checks=$((checks + 1))
    warnings=$((warnings + 1))
    printf '%s⚠%s %s\n' "$orange" "$reset" "$1"
}

critical() {
    checks=$((checks + 1))
    criticals=$((criticals + 1))
    printf '%s✗%s %s\n' "$red" "$reset" "$1"
}

info() {
    printf '%si%s %s\n' "$cyan" "$reset" "$1"
}

section() {
    printf '\n%s== %s ==%s\n' "$bold" "$1" "$reset"
}

section 'Сервер'
info "Время сервера: $(date '+%Y-%m-%d %H:%M:%S %Z')"
info "Хост: $(hostname)"
info "Uptime: $(uptime 2>/dev/null || printf 'недоступен')"

if [ ! -d "$app_dir" ]; then
    critical "Каталог $app_dir не найден"
    printf '\nИтог: CRITICAL (%s критических, %s предупреждений, %s проверок)\n' \
        "$criticals" "$warnings" "$checks"
    exit 2
fi
ok "Каталог приложения существует: $app_dir"

if [ -s "$app_dir/.env" ]; then
    ok '.env существует и не пустой'
else
    critical '.env отсутствует или пустой'
fi

db_path="$app_dir/data/accountant.db"
if [ -s "$db_path" ]; then
    db_size=$(du -h "$db_path" 2>/dev/null | awk '{print $1}')
    ok "SQLite-база существует (${db_size:-размер неизвестен})"
else
    critical "SQLite-база отсутствует или пустая: $db_path"
fi

section 'Ресурсы'
disk_data=$(df -P "$app_dir" 2>/dev/null | awk 'NR == 2 {gsub(/%/, "", $5); print $5 "|" $4}')
disk_used=${disk_data%%|*}
disk_free_kb=${disk_data#*|}
case "$disk_used" in
    ''|*[!0-9]*) warn 'Не удалось определить заполнение диска' ;;
    *)
        if [ "$disk_used" -ge 90 ]; then
            critical "Диск заполнен на ${disk_used}% (свободно ${disk_free_kb} KiB)"
        elif [ "$disk_used" -ge 80 ]; then
            warn "Диск заполнен на ${disk_used}% (свободно ${disk_free_kb} KiB)"
        else
            ok "Диск заполнен на ${disk_used}% (свободно ${disk_free_kb} KiB)"
        fi
        ;;
esac

if command -v free >/dev/null 2>&1; then
    memory_data=$(free -m | awk '/^Mem:/ {print $2 "|" $7}')
    memory_total=${memory_data%%|*}
    memory_available=${memory_data#*|}
    if [ -n "$memory_total" ] && [ "$memory_total" -gt 0 ]; then
        memory_available_percent=$((memory_available * 100 / memory_total))
        if [ "$memory_available_percent" -lt 10 ]; then
            warn "Мало доступной памяти: ${memory_available} MiB из ${memory_total} MiB (${memory_available_percent}%)"
        else
            ok "Доступная память: ${memory_available} MiB из ${memory_total} MiB (${memory_available_percent}%)"
        fi
    else
        warn 'Не удалось определить доступную память'
    fi
else
    info 'Команда free отсутствует — проверка памяти пропущена'
fi

section 'Docker и приложение'
if ! command -v docker >/dev/null 2>&1; then
    critical 'Docker не установлен'
else
    if docker info >/dev/null 2>&1; then
        ok 'Docker daemon отвечает'
    else
        critical 'Docker daemon недоступен'
    fi

    if [ -f "$app_dir/compose.yaml" ] \
        && docker compose -f "$app_dir/compose.yaml" --env-file "$app_dir/.env" config --quiet >/dev/null 2>&1; then
        ok 'Docker Compose-конфигурация корректна'
    else
        critical 'Docker Compose-конфигурация некорректна или отсутствует'
    fi

    inspect=$(docker inspect --format '{{.State.Status}}|{{.State.Running}}|{{.RestartCount}}|{{.State.StartedAt}}' "$container" 2>/dev/null)
    if [ -z "$inspect" ]; then
        critical "Контейнер $container не найден"
    else
        state=$(printf '%s' "$inspect" | cut -d '|' -f 1)
        running=$(printf '%s' "$inspect" | cut -d '|' -f 2)
        restarts=$(printf '%s' "$inspect" | cut -d '|' -f 3)
        started_at=$(printf '%s' "$inspect" | cut -d '|' -f 4)
        if [ "$state" = 'running' ] && [ "$running" = 'true' ]; then
            ok "Контейнер работает (запущен $started_at)"
        else
            critical "Контейнер не работает: state=$state running=$running"
        fi
        case "$restarts" in
            ''|*[!0-9]*) warn 'Не удалось определить число перезапусков контейнера' ;;
            0) ok 'Самопроизвольных перезапусков контейнера нет' ;;
            1|2|3) warn "Контейнер перезапускался: $restarts" ;;
            *) critical "Слишком много перезапусков контейнера: $restarts" ;;
        esac

        recent_logs=$(docker logs --since "$log_since" --tail 1000 "$container" 2>&1)
        error_count=$(printf '%s\n' "$recent_logs" | grep -Eic '"level":"ERROR"|panic|fatal|thread .* panicked' || true)
        warning_count=$(printf '%s\n' "$recent_logs" | grep -Ec '"level":"WARN"' || true)
        reconnect_count=$(printf '%s\n' "$recent_logs" | grep -Eic 'connection interrupted|requested reconnection|failed to obtain VK Long Poll' || true)

        if [ "$error_count" -gt 0 ]; then
            warn "В логах за $log_since найдено ошибок: $error_count"
            printf '%s\n' "$recent_logs" \
                | grep -Ei '"level":"ERROR"|panic|fatal|thread .* panicked' \
                | tail -n 5 \
                | while IFS= read -r error_line; do
                    printf '%s  %s%s\n' "$red" "$error_line" "$reset"
                done
        else
            ok "Ошибок и panic в логах за $log_since нет"
        fi

        if [ "$warning_count" -gt 10 ]; then
            warn "Много предупреждений в логах за $log_since: $warning_count"
        else
            info "Предупреждений в логах за $log_since: $warning_count"
        fi

        if [ "$reconnect_count" -gt 3 ]; then
            warn "Частые переподключения VK Long Poll за $log_since: $reconnect_count"
        else
            info "Переподключений VK Long Poll за $log_since: $reconnect_count"
        fi

        startup_logs=$(docker logs --tail 1000 "$container" 2>&1)
        if printf '%s\n' "$startup_logs" | grep -q 'connected to VK Long Poll'; then
            ok 'В логах подтверждено подключение к VK Long Poll'
        else
            warn 'В последних 1000 строках нет подтверждения подключения к VK Long Poll'
        fi
    fi
fi

section 'SQLite и очередь сообщений'
if [ ! -s "$db_path" ]; then
    critical 'Проверка SQLite невозможна: база отсутствует'
elif ! command -v sqlite3 >/dev/null 2>&1; then
    warn 'sqlite3 не установлен — проверка базы и outbox пропущена'
else
    quick_check=$(sqlite3 -readonly "$db_path" 'PRAGMA quick_check;' 2>&1)
    if [ "$quick_check" = 'ok' ]; then
        ok 'PRAGMA quick_check: база цела'
    else
        critical "SQLite quick_check завершился ошибкой: $quick_check"
    fi

    pending=$(sqlite3 -readonly "$db_path" "SELECT COUNT(*) FROM outbox WHERE sent_at IS NULL;" 2>/dev/null)
    stuck=$(sqlite3 -readonly "$db_path" "SELECT COUNT(*) FROM outbox WHERE sent_at IS NULL AND created_at <= datetime('now', '-10 minutes');" 2>/dev/null)
    repeatedly_failed=$(sqlite3 -readonly "$db_path" "SELECT COUNT(*) FROM outbox WHERE sent_at IS NULL AND attempt_count >= 5;" 2>/dev/null)
    operations=$(sqlite3 -readonly "$db_path" "SELECT COUNT(*) FROM operations WHERE cancelled_by_event_id IS NULL;" 2>/dev/null)
    conversations=$(sqlite3 -readonly "$db_path" "SELECT COUNT(DISTINCT peer_id) FROM operations;" 2>/dev/null)

    case "$pending" in
        ''|*[!0-9]*) warn 'Не удалось прочитать очередь outbox' ;;
        0) ok 'Очередь outbox пуста' ;;
        *) warn "В outbox ожидают отправки сообщений: $pending" ;;
    esac
    case "$stuck" in
        ''|*[!0-9]*) warn 'Не удалось проверить зависшие сообщения outbox' ;;
        0) ok 'Сообщений старше 10 минут в outbox нет' ;;
        *) warn "В outbox есть сообщения старше 10 минут: $stuck" ;;
    esac
    case "$repeatedly_failed" in
        ''|*[!0-9]*) warn 'Не удалось проверить повторные ошибки outbox' ;;
        0) ok 'Сообщений с 5+ неудачными попытками нет' ;;
        *) critical "Сообщений с 5+ неудачными попытками: $repeatedly_failed" ;;
    esac
    info "Активных операций: ${operations:-неизвестно}; бесед с операциями: ${conversations:-неизвестно}"
fi

section 'Резервные копии'
if command -v systemctl >/dev/null 2>&1; then
    if systemctl is-active --quiet accountant-bot-backup.timer; then
        ok 'systemd-таймер резервного копирования активен'
    else
        warn 'systemd-таймер accountant-bot-backup.timer не активен'
    fi
    if systemctl is-failed --quiet accountant-bot-backup.service; then
        critical 'Последний запуск accountant-bot-backup.service завершился ошибкой'
    else
        ok 'Сервис резервного копирования не находится в failed-state'
    fi
else
    warn 'systemctl недоступен — таймер резервного копирования не проверен'
fi

backup_dir="$app_dir/backups"
latest_backup=''
if [ -d "$backup_dir" ]; then
    latest_backup=$(find "$backup_dir" -maxdepth 1 -type f -name 'accountant-*.db' -print 2>/dev/null | sort | tail -n 1)
fi
if [ -z "$latest_backup" ]; then
    warn "Резервные копии не найдены в $backup_dir"
else
    backup_epoch=$(stat -c %Y "$latest_backup" 2>/dev/null)
    now_epoch=$(date +%s)
    if [ -n "$backup_epoch" ]; then
        backup_age_hours=$(((now_epoch - backup_epoch) / 3600))
        backup_size=$(du -h "$latest_backup" 2>/dev/null | awk '{print $1}')
        if [ "$backup_age_hours" -gt $((backup_max_age_hours * 2)) ]; then
            critical "Последний бэкап слишком старый: ${backup_age_hours} ч., файл $(basename "$latest_backup") (${backup_size:-?})"
        elif [ "$backup_age_hours" -gt "$backup_max_age_hours" ]; then
            warn "Последний бэкап старше нормы: ${backup_age_hours} ч., файл $(basename "$latest_backup") (${backup_size:-?})"
        else
            ok "Последний бэкап свежий: ${backup_age_hours} ч., файл $(basename "$latest_backup") (${backup_size:-?})"
        fi
    else
        warn 'Не удалось определить возраст последнего бэкапа'
    fi

    if command -v sqlite3 >/dev/null 2>&1; then
        backup_check=$(sqlite3 -readonly "$latest_backup" 'PRAGMA quick_check;' 2>&1)
        if [ "$backup_check" = 'ok' ]; then
            ok 'Последний бэкап проходит SQLite quick_check'
        else
            critical "Последний бэкап повреждён: $backup_check"
        fi
    fi
fi

printf '\n'
if [ "$criticals" -gt 0 ]; then
    printf '%s✗ Итог: CRITICAL (%s критических, %s предупреждений, %s проверок)%s\n' \
        "$red" "$criticals" "$warnings" "$checks" "$reset"
    exit 2
fi
if [ "$warnings" -gt 0 ]; then
    printf '%s⚠ Итог: WARNING (0 критических, %s предупреждений, %s проверок)%s\n' \
        "$orange" "$warnings" "$checks" "$reset"
    exit 1
fi
printf '%s✓ Итог: OK (все %s проверок пройдены)%s\n' "$green" "$checks" "$reset"
exit 0
REMOTE_CHECK
status=$?

if [ "$status" -eq 255 ]; then
    printf 'CRITICAL: не удалось подключиться к %s по SSH\n' "$target" >&2
    exit 2
fi
exit "$status"
