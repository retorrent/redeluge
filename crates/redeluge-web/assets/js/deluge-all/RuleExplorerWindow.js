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
        this.labelBox.on('select', this.draw, this);
        this.trackerBox.on('select', this.draw, this);

        this.add({
            region: 'north',
            xtype: 'form',
            border: false,
            height: 64,
            labelWidth: 130,
            bodyStyle: 'padding: 8px 10px 0 10px;',
            items: [this.labelBox, this.trackerBox],
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
        var trackers = [['', _('(a tracker with no rule)')]];
        Ext.each(Ext.keys((this.cfg.tracker || {}).trackers || {}).sort(), function (host) {
            trackers.push([host, host]);
        });
        keep(this.trackerBox, trackers);
    },

    /** The life of a torrent with this label from this tracker, as stages. */
    explore: function (startLabel, host) {
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
        var src = {
            label: function (name) { return String.format(_('label {0}'), name || _('(none)')); },
            tracker: String.format(_('tracker {0}'), host),
            daemon: _('the daemon'),
        };
        var stages = [];
        var stage = function (title) {
            var s = { title: title, items: [] };
            stages.push(s);
            return s;
        };
        var item = function (s, from, when, what, kind) {
            s.items.push({ from: from, when: when, what: what, kind: kind || 'rule' });
        };

        var label = startLabel;
        var opts = function (name) { return name ? labels[name] : null; };

        // -------------------------------------------------------- arriving
        var s = stage(_('Arrives'));
        if (tracker && tracker.block) {
            item(s, src.tracker, _('when added'), _('refused or held: this tracker is blocked'), 'warn');
        }
        if (tracker && tracker.auto_label && tracker.label && tracker.label_on_add && !label) {
            label = tracker.label;
            item(s, src.tracker, _('when added'), String.format(_('goes in label {0}'), label));
        }
        item(s, src.label(label), _('from now on'), label ? String.format(_('the rules of label {0} apply'), label) : _('no label: only the tracker\'s and the daemon\'s rules apply'), 'info');
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

        // ------------------------------------------------------- finishing
        s = stage(_('Once finished'));
        var timed = [];
        if (lo && lo.apply_move_completed && lo.move_completed && lo.move_completed_path) {
            timed.push({ h: 0, from: src.label(label), when: _('as soon as it finishes'), what: String.format(_('moved to {0}'), lo.move_completed_path) });
        }
        if (tracker && tracker.auto_label && tracker.label && tracker.label_when_done) {
            timed.push({ h: tracker.label_after_hours || 0, from: src.tracker, when: after(tracker.label_after_hours), what: String.format(_('put in label {0}'), tracker.label), relabel: tracker.label });
        }
        if (tracker && tracker.auto_move && tracker.move_path) {
            timed.push({ h: tracker.move_after_hours || 0, from: src.tracker, when: after(tracker.move_after_hours), what: String.format(_('moved to {0} — unless it is on an archive by then, which the archive keeps'), tracker.move_path) });
        }
        timed.sort(function (a, b) { return a.h - b.h; });
        Ext.each(timed, function (t) {
            item(s, t.from, t.when, t.what);
            if (t.relabel && t.relabel !== label) {
                var wasProtected = protect;
                var before = label;
                label = t.relabel;
                lo = opts(label);
                protect = lo && lo.never_remove;
                if (wasProtected && !protect) {
                    item(s, src.label(label), _('from then on'), String.format(_('label {0} does not protect it: the never-remove of label {1} no longer applies'), label, before), 'warn');
                }
                item(s, src.label(label), _('from then on'), String.format(_('the rules of label {0} apply instead'), label), 'info');
            }
        });
        if (lo && lo.apply_queue && lo.stop_at_ratio) {
            item(s, src.label(label), String.format(_('at share ratio {0}'), lo.stop_ratio),
                lo.remove_at_ratio && !protect ? _('removed (files kept)') : _('paused'),
                lo.remove_at_ratio && !protect ? 'danger' : 'rule');
        }
        if (!s.items.length) item(s, '', '', _('nothing acts on it: it seeds'), 'info');

        // --------------------------------------------------------- archive
        s = stage(_('Archive'));
        var targets = archive.targets || [];
        if (lo && lo.apply_archive && lo.archive_target) {
            var target = null;
            Ext.each(targets, function (t) { if (t.name === lo.archive_target) target = t; });
            if (!target) {
                item(s, src.label(label), '', String.format(_('archives to {0}, which is not a target in Preferences, Archive: nothing happens'), lo.archive_target), 'warn');
            } else {
                item(s, src.label(label), String.format(_('once it has sent and received nothing for {0}'), hours(lo.archive_after_hours)),
                    String.format(_('paused and copied to {0} ({1})'), target.name, target.path));
                if (target.rc_url) {
                    item(s, _('rclone'), _('then'), _('uploaded to the bucket while it stays paused (Uploading); if the upload fails: Sync Error, at the back of the queue, retried when nothing else is waiting'));
                } else {
                    item(s, _('rclone'), _('then'), _('no rc URL: redeluge cannot see the upload, and lets it go as soon as the copy is done'), 'warn');
                }
                item(s, src.label(label), _('after the upload'), _('seeds from the archive (Remote Seed); paused as Remote Error whenever the mount does not answer'));
            }
        } else if (lo && lo.apply_archive) {
            item(s, src.label(label), '', _('Archive is on but no target is chosen: nothing happens'), 'warn');
        } else {
            item(s, '', '', _('its label does not archive: it stays where it is'), 'info');
            if (lo && lo.restore_local) {
                item(s, src.label(label), _('if it is on an archive'), _('brought back to local disk, one at a time, if there is room'));
            }
        }

        // ------------------------------------------------------------- end
        s = stage(_('How it goes'));
        if (tracker && tracker.auto_remove) {
            if (protect) {
                item(s, src.tracker, after(tracker.remove_after_hours), _('would be removed, but its label never removes: kept'), 'info');
            } else {
                item(s, src.tracker, after(tracker.remove_after_hours),
                    tracker.remove_data ? _('removed with its files — deleted from the bucket too if it is archived') : _('removed, files kept'), 'danger');
            }
        }
        if (!s.items.length) item(s, '', '', _('nothing removes it: it stays until removed by hand'), 'info');
        return stages;
    },

    draw: function () {
        if (!this.cfg) return;
        var esc = Ext.util.Format.htmlEncode;
        var colours = { danger: '#e6381f', warn: '#f0883e', info: 'inherit', rule: 'inherit' };
        var stages = this.explore(this.labelBox.getValue() || '', this.trackerBox.getValue() || '');
        var html = stages.map(function (s, n) {
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
                '<div style="margin: 8px 0 0 0; border-left: 3px solid #f0883e; padding: 2px 0 4px 10px;">' +
                '<div style="font-weight: bold; margin-bottom: 3px;">' + (n + 1) + '. ' + esc(s.title) + '</div>' +
                '<table>' + rows + '</table></div>'
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
