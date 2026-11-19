#!/bin/sh
set -eu
root=${0%/*}
test "$1" = --json || exit 9
case "$2" in
    plan)
        for argument do
            if [ "${previous:-}" = --home ]; then home=$argument; fi
            previous=$argument
            folder=$argument
        done
        test "$folder" = "$PWD" || exit 9
        ref=$(/bin/cat "$folder/fixture-ref")
        printf '{"apiVersion":"brig.sh/v1alpha1","kind":"Plan","data":{"profile":"%s","session":"%s","sandbox":"brig-%s","runtime":{"available":true,"kind":"hull","backend":"hvi"},"project":{"host":"%s","guest":"/work/fixture"},"home":{"host":"%s-%s","guest":"/root","mode":"read-write"},"posture":"isolated","ports":%s}}\n' \
            "${ref%@*}" "${ref#*@}" "$ref" "$folder" "$home" "${ref#*@}" "$(/bin/cat "$root/$ref-plan-ports")"
        ;;
    ls) exec /bin/cat "$root/inventory" ;;
    info) exec /bin/cat "$root/$3-info" ;;
    network)
        verb=$3
        ref=$4
        test "$ref" = "$(/bin/cat "$PWD/fixture-ref")" || exit 9
        case "$verb" in
            ls) test "$#" = 4 ;;
            publish)
                test "$#" = 5
                guest=${5##*:}
                address=${5%:*}
                test "${address%:*}" = '127.0.0.1'
                printf '%s %s %s\n' "$verb" "$ref" "$5" >> "$root/mutations"
                printf '["%s"]' "$5" > "$root/$ref-plan-ports"
                printf '[{"host":"%s","guest":%s,"protocol":"tcp","live":false}]' "$address" "$guest" > "$root/$ref-ports"
                printf '%s' "$address" > "$root/$ref-address"
                test ! -f "$root/$ref-interrupt-publish"
                ;;
            unpublish)
                test "$#" = 5
                test "$5" = "$(/bin/cat "$root/$ref-address")"
                printf '%s %s %s\n' "$verb" "$ref" "$5" >> "$root/mutations"
                printf '[]' > "$root/$ref-plan-ports"
                printf '[]' > "$root/$ref-ports"
                test ! -f "$root/$ref-interrupt-unpublish"
                ;;
            *) exit 9 ;;
        esac
        printf '{"apiVersion":"brig.sh/v1alpha1","kind":"Ports","data":{"sandbox":"brig-%s","ports":%s}}\n' "$ref" "$(/bin/cat "$root/$ref-ports")"
        ;;
    *) exit 9 ;;
esac
