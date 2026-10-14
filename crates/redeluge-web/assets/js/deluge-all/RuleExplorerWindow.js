/**
 * Deluge.RuleExplorerWindow.js
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * What would happen to a torrent, from the rules as they are set now: pick the
 * label it arrives with and the tracker it comes from, and this lays out its
 * life — arriving, downloading, finishing, archiving, going — with every rule
 * that acts on it, who sets that rule (a label, a tracker, the daemon) and
 * when it fires. It follows the chain too: a tracker rule that moves it into
 * another label hands it over to that label's rules from then on.
 *
 * It reads the settings only (`label.get_config`, `core.get_config_value`)
 * and computes everything here: nothing it shows has happened yet.
 */
Ext.ns('Deluge');

/**
 * @class Deluge.RuleExplorerWindow
 * @extends Ext.Window
 */
Deluge.RuleExplorerWindow = Ext.extend(Ext.Window, {
    title: _('Rule Explorer'),
    width: 760,
    height: 600,
    layout: 'border',
    closeAction: 'hide',
    constrainHeader: true,
    plain: true,
    minWidth: 520,
    minHeight: 360,

    initComponent: function () {
        Deluge.RuleExplorerWindow.superclass.initComponent.call(this);

        var combo = function (label, emptyText) {
            return new Ext.form.ComboBox({
                fieldLabel: label,
                store: new Ext.data.ArrayStore({ fields: ['value', 'text'], data: [] }),
                valueField: 'value',
                displayField: 'text',
                mode: 'local',
                triggerAction: 'all',
                editable: false,
                width: 240,
                emptyText: emptyText,
            });
        };
        this.labelBox = combo(_('Arrives with label'), _('(no label)'));
        this.trackerBox = combo(_('From tracker'), _('(a tracker with no rule)'));
        // A label change no rule here makes: somebody by hand, or an *arr's
        // post-import category. What follows it is simulated like the rest.
        this.laterBox = combo(_('Later put in label'), _('(nobody changes it)'));
        this.laterHours = new Ext.form.NumberField({
            fieldLabel: _('…hours after it finishes'),
            width: 80,
            value: 0,
            minValue: 0,
            allowDecimals: true,
        });
        this.labelBox.on('select', this.draw, this);
        this.trackerBox.on('select', this.draw, this);
        this.laterBox.on('select', this.draw, this);
        this.laterHours.on('change', this.draw, this);

        this.add({
            region: 'north',
            xtype: 'form',
            border: false,
            height: 116,
            labelWidth: 170,
            bodyStyle: 'padding: 8px 10px 0 10px;',
            items: [this.labelBox, this.trackerBox, this.laterBox, this.laterHours],
        });
        this.flow = this.add({
            region: 'center',
            xtype: 'panel',
            border: false,
            autoScroll: true,
            bodyStyle: 'padding: 6px 12px 12px 12px;',
            html: '',
        });

        this.addButton(_('Reload rules'), this.load, this);
        this.addButton(_('Close'), this.hide, this);
        this.on('show', this.load, this);
    },

    /** Reads every setting a rule lives in, then draws. */
    load: function () {
        var cfg = {};
        var pending = 5;
        var done = function () {
            if (--pending > 0) return;
            this.cfg = cfg;
            this.fillChoices();
            this.draw();
        };
        deluge.client.label.get_config({
            success: function (value) {
                cfg.labels = (value && value.labels) || {};
                done.call(this);
            },
            failure: done,
            scope: this,
        });
        Ext.each(['tracker', 'stuck', 'archive', 'idle_pause'], function (key) {
            deluge.client.core.get_config_value(key, {
                success: function (value) {
                    cfg[key] = value || {};
                    done.call(this);
                },
                failure: done,
                scope: this,
            });
        }, this);
    },

    fillChoices: function () {
        var keep = function (box, rows) {
            var value = box.getValue();
            box.getStore().loadData(rows);
            if (value !== undefined) box.setValue(value);
        };
        var labels = [['', _('(no label)')]];
        Ext.each(Ext.keys(this.cfg.labels || {}).sort(), function (name) {
            labels.push([name, name]);
        });
        keep(this.labelBox, labels);
        keep(
            this.laterBox,
            [['', _('(nobody changes it)')]].concat(labels.slice(1))
        );
        var trackers = [['', _('(a tracker with no rule)')]];
        Ext.each(Ext.keys((this.cfg.tracker || {}).trackers || {}).sort(), function (host) {
            trackers.push([host, host]);
        });
        keep(this.trackerBox, trackers);
    },

    /** The life of a torrent with this label from this tracker, as stages. */
    explore: function (startLabel, host, later, laterHours) {
        var cfg = this.cfg;
        var labels = cfg.labels || {};
        var tracker = host ? ((cfg.tracker || {}).trackers || {})[host] : null;
        var stuck = cfg.stuck || {};
        var archive = cfg.archive || {};
        var idle = cfg.idle_pause || {};
        var hours = function (h) {
            return h > 0 ? ftime(h * 3600) : _('at once');
        };
        var after = function (h, what) {
            return h > 0 ? String.format(_('{0} after it finishes'), ftime(h * 3600)) : what || _('as soon as it finishes');
        };
        // A label's name, marked so that drawing shows it as <b>"name"</b>.
        var L = function (name) { return '\u0001' + name + '\u0002'; };
        var src = {
            label: function (name) { return name ? String.format(_('label {0}'), L(name)) : _('no label'); },
            tracker: String.format(_('tracker {0}'), host),
            daemon: _('the daemon'),
        };
        // One segment per label the torrent is in, in order: its stages are
        // numbered from one and drawn in that label's colour.
        var segments = [];
        var segment = function (name, since) {
            segments.push({ label: name, since: since, stages: [] });
        };
        var stage = function (title) {
            var s = { title: title, items: [] };
            segments[segments.length - 1].stages.push(s);
            return s;
        };
        var item = function (s, from, when, what, kind) {
            s.items.push({ from: from, when: when, what: what, kind: kind || 'rule' });
        };

        // The daemon stores labels lowercased, without spaces or punctuation:
        // a tracker rule that says "Films" files the torrent under "films".
        var norm = function (raw) {
            return String(raw || '').trim().toLowerCase().replace(/[^a-z0-9_.-]/g, '');
        };
        var label = norm(startLabel);
        var opts = function (name) { return name ? labels[name] : null; };

        // -------------------------------------------------------- arriving
        segment(label, _('when added'));
        var s = stage(_('Arrives'));
        if (tracker && tracker.block) {
            item(s, src.tracker, _('when added'), _('refused or held: this tracker is blocked'), 'warn');
        }
        if (tracker && tracker.auto_label && tracker.label && tracker.label_on_add && !label) {
            label = norm(tracker.label);
            segments[0].label = label;
            item(s, src.tracker, _('when added'), String.format(_('goes in label {0}'), L(label)));
        }
        item(s, src.label(label), _('from now on'), label ? String.format(_('the rules of label {0} apply'), L(label)) : _('no label: only the tracker\'s and the daemon\'s rules apply'), 'info');
        if (label && !labels[label]) {
            item(s, src.label(label), '', _('this label does not exist: nothing of it applies'), 'warn');
        }
        if (opts(label) && opts(label).apply_max) {
            item(s, src.label(label), _('all along'), _('bandwidth and connection limits of the label'));
        }
        if (tracker && tracker.auto_limit) {
            item(s, src.tracker, _('all along'), _('bandwidth and connection limits of the tracker'));
        }

        // ------------------------------------------------------ downloading
        s = stage(_('While downloading'));
        var lo = opts(label);
        var protect = lo && lo.never_remove;
        var stuckRule = lo && lo.apply_stuck
            ? { from: src.label(label), h: lo.stuck_hours, action: lo.stuck_action, data: lo.stuck_remove_data, dead: lo.stuck_dead_only, max: lo.stuck_max_progress }
            : stuck.enabled
              ? { from: src.daemon, h: stuck.hours, action: stuck.action, data: stuck.remove_data, dead: stuck.dead_only, max: stuck.max_progress }
              : null;
        if (stuckRule) {
            var removes = stuckRule.action === 'remove' && !protect;
            item(
                s,
                stuckRule.from,
                String.format(_('after {0} without receiving a byte'), hours(stuckRule.h)) +
                    (stuckRule.dead ? _(', if the tracker says nobody has it') : '') +
                    (stuckRule.max > 0 ? String.format(_(', up to {0}% done'), stuckRule.max) : _(', if it never started')),
                removes
                    ? (stuckRule.data ? _('removed with its files') : _('removed, files kept'))
                    : _('paused') + (stuckRule.action === 'remove' ? _(' (its label never removes)') : ''),
                removes ? 'danger' : 'rule'
            );
        }
        if (tracker && tracker.remove_when_down) {
            item(s, src.tracker, String.format(_('if the tracker is down for {0}'), hours(tracker.remove_down_hours)),
                protect ? _('kept: its label never removes') : _('removed with its files'), protect ? 'rule' : 'danger');
        }
        if (idle.enabled) {
            item(s, src.daemon, _('when idle while others are queued'), _('paused for a while so the queue moves'));
        }
        if (!s.items.length) item(s, '', '', _('nothing acts on it: it downloads'), 'info');

        // ------------------------------------------------- after finishing
        // A timeline, not a list: each event is applied in the order of its
        // hour, and what follows sees the torrent as that event left it —
        // its label, and whether it is on an archive. A label that archives
        // at once and a tracker rule that relabels it twelve hours later is a
        // torrent already on the archive when its new label takes over.
        s = stage(_('Once finished, in order'));
        var targets = archive.targets || [];
        var targetNamed = function (name) {
            var found = null;
            Ext.each(targets, function (t) { if (t.name === name) found = t; });
            return found;
        };
        var at = function (h) {
            return h > 0 ? String.format(_('{0} after it finishes'), ftime(h * 3600)) : _('as soon as it finishes');
        };
        var on = null; // the target it is on, or null for local disk

        if (lo && lo.apply_move_completed && lo.move_completed && lo.move_completed_path) {
            item(s, src.label(label), at(0), String.format(_('moved to {0}'), lo.move_completed_path));
        }
        if (lo && lo.apply_queue && lo.stop_at_ratio) {
            item(s, src.label(label), String.format(_('at share ratio {0}'), lo.stop_ratio),
                lo.remove_at_ratio && !protect ? _('removed (files kept)') : _('paused'),
                lo.remove_at_ratio && !protect ? 'danger' : 'rule');
        }

        var pending = [];
        if (tracker && tracker.auto_label && tracker.label && tracker.label_when_done) {
            pending.push({ h: tracker.label_after_hours || 0, kind: 'relabel', to: norm(tracker.label), from: src.tracker });
        }
        if (later) {
            pending.push({ h: laterHours || 0, kind: 'relabel', to: norm(later), from: _('a person or an *arr') });
        }
        if (tracker && tracker.auto_move && tracker.move_path) {
            pending.push({ h: tracker.move_after_hours || 0, kind: 'move', to: tracker.move_path, from: src.tracker });
        }
        if (tracker && tracker.auto_remove) {
            pending.push({ h: tracker.remove_after_hours || 0, kind: 'remove', from: src.tracker });
        }

        // When its current label would archive it, if it is not already there.
        // Labels whose archive rule cannot act (no target, unknown target),
        // already said once: not said again on every pass.
        var noArchive = {};
        var archiveEvent = function (now) {
            var o = opts(label);
            if (!o || !o.apply_archive || noArchive[label]) return null;
            if (!o.archive_target) return { h: now, kind: 'archive-missing' };
            if (on && on.name === o.archive_target) return null;
            return { h: Math.max(now, o.archive_after_hours || 0), kind: 'archive', target: o.archive_target, idle: o.archive_after_hours || 0 };
        };

        var now = 0;
        var removed = false;
        var guard = 0;
        while (!removed && guard++ < 20) {
            var next = archiveEvent(now);
            pending.sort(function (a, b) { return a.h - b.h; });
            // The archive first on a tie: the sweep runs every fifteen seconds,
            // more often than any rule measured in hours.
            if (pending.length && (!next || pending[0].h < next.h)) next = pending.shift();
            if (!next) break;
            now = next.h;

            if (next.kind === 'archive-missing') {
                item(s, src.label(label), '', _('Archive is on but no target is chosen: nothing happens'), 'warn');
                noArchive[label] = true;
            } else if (next.kind === 'archive') {
                var target = targetNamed(next.target);
                if (!target) {
                    item(s, src.label(label), '', String.format(_('archives to {0}, which is not a target in Preferences, Archive: nothing happens'), next.target), 'warn');
                    noArchive[label] = true;
                    continue;
                }
                item(s, src.label(label),
                    next.idle > 0 ? String.format(_('once idle for {0} (from {1})'), ftime(next.idle * 3600), at(now)) : at(now),
                    String.format(on ? _('moved from {0} to {1} ({2}), paused while rclone uploads it, then seeds from there') : _('paused and copied to {1} ({2}), uploaded while it stays paused, then seeds from there (Remote Seed)'),
                        on && on.name, target.name, target.path));
                if (!target.rc_url) {
                    item(s, _('rclone'), '', String.format(_('{0} has no rc URL: redeluge cannot see the upload and lets it go as soon as the copy is done'), target.name), 'warn');
                }
                on = target;
            } else if (next.kind === 'relabel') {
                if (next.to === label) continue;
                var before = label;
                var wasProtected = protect;
                label = next.to;
                lo = opts(label);
                protect = lo && lo.never_remove;
                item(s, next.from, at(now), String.format(_('put in label {0}'), L(label)));
                // A new label, a new segment: what follows is that label's.
                segment(label, String.format(_('{0}, by {1}'), at(now), next.from));
                s = stage(_('From then on, in order'));
                if (!labels[label]) {
                    item(s, src.label(label), '', _('this label does not exist: none of its rules apply'), 'warn');
                }
                if (wasProtected && !protect) {
                    item(s, src.label(label), '', String.format(_('label {0} does not protect it: the never-remove of label {1} no longer applies'), L(label), L(before)), 'warn');
                }
                if (on && !(lo && lo.apply_archive)) {
                    if (lo && lo.restore_local) {
                        item(s, src.label(label), at(now), String.format(_('brought back from {0} to local disk, if there is room'), on.name));
                        on = null;
                    } else {
                        item(s, src.label(label), at(now), String.format(_('stays on {0}: this label does not archive and does not bring torrents back'), on.name), 'info');
                    }
                }
            } else if (next.kind === 'move') {
                if (on) {
                    item(s, next.from, at(now), String.format(_('would move it to {0}, but it is on {1}: the archive keeps it'), next.to, on.name), 'info');
                } else {
                    item(s, next.from, at(now), String.format(_('moved to {0}'), next.to));
                }
            } else if (next.kind === 'remove') {
                if (protect) {
                    item(s, next.from, at(now), _('would be removed, but its label never removes: kept'), 'info');
                } else {
                    item(s, next.from, at(now),
                        tracker.remove_data
                            ? (on ? String.format(_('removed with its files — deleted from {0} too'), on.name) : _('removed with its files'))
                            : _('removed, files kept'),
                        'danger');
                    removed = true;
                }
            }
        }
        if (!removed) {
            item(s, '', '', on
                ? String.format(_('stays on {0}, seeding, until removed by hand'), on.name)
                : _('stays on local disk until removed by hand'), 'info');
        }
        return segments;
    },

    draw: function () {
        if (!this.cfg) return;
        var esc = function (text) {
            return Ext.util.Format.htmlEncode(text).replace(
                /\u0001([^\u0002]*)\u0002/g,
                '<b>"$1"</b>'
            );
        };
        var colours = { danger: '#e6381f', warn: '#f0883e', info: 'inherit', rule: 'inherit' };
        var segments = this.explore(
            this.labelBox.getValue() || '',
            this.trackerBox.getValue() || '',
            this.laterBox.getValue() || '',
            Number(this.laterHours.getValue()) || 0
        );
        // One colour per label period, in order, so a change of label is a
        // change of colour down the left edge.
        var palette = ['#f0883e', '#58a6ff', '#3fb950', '#d2a8ff', '#e3b341', '#ff7b72'];
        var html = segments.map(function (g, k) {
            var colour = palette[k % palette.length];
            var stages = g.stages.map(function (s, n) {
                var rows = s.items.map(function (it) {
                    return (
                        '<tr>' +
                        '<td style="white-space: nowrap; opacity: 0.72; padding: 2px 12px 2px 0; vertical-align: top;">' + esc(it.from || '') + '</td>' +
                        '<td style="white-space: nowrap; padding: 2px 12px 2px 0; vertical-align: top;">' + esc(it.when || '') + '</td>' +
                        '<td style="padding: 2px 0; vertical-align: top; color: ' + colours[it.kind] + ';' + (it.kind === 'info' ? ' font-style: italic; opacity: 0.85;' : '') + '">' + esc(it.what) + '</td>' +
                        '</tr>'
                    );
                }).join('');
                return (
                    '<div style="margin: 6px 0 0 0; padding: 0 0 2px 0;">' +
                    '<div style="font-weight: bold; margin-bottom: 3px;">' + (n + 1) + '. ' + esc(s.title) + '</div>' +
                    '<table>' + rows + '</table></div>'
                );
            }).join('');
            return (
                '<div style="margin: 10px 0 0 0; border-left: 4px solid ' + colour + '; padding: 2px 0 6px 12px;">' +
                '<div style="font-weight: bold; font-size: 1.1em; color: ' + colour + ';">' +
                (g.label ? _('Label') + ' <b>"' + Ext.util.Format.htmlEncode(g.label) + '"</b>' : esc(_('No label'))) +
                '<span style="font-weight: normal; font-size: 0.9em; opacity: 0.8; color: inherit;"> — ' + esc(g.since) + '</span></div>' +
                stages + '</div>'
            );
        }).join('');
        this.flow.update(html);
    },
});

/** The one window, built when the menu first asks for it. */
Deluge.RuleExplorerWindow.open = function () {
    if (!deluge.ruleExplorer) deluge.ruleExplorer = new Deluge.RuleExplorerWindow();
    deluge.ruleExplorer.show();
};
