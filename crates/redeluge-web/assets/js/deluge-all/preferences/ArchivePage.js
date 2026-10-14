/**
 * Deluge.preferences.ArchivePage.js
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Archive targets: the mounts (usually S3 through rclone) a label can send its
 * quiet torrents to. The top half edits the list; the bottom half shows each
 * target as it really is — whether it answers, and what rclone says it mounts
 * and how its cache is set up. Secrets never leave rclone: the daemon only
 * passes on the remote's type, provider, endpoint and region.
 *
 * `archive` in core.conf, and `redeluge.get_archive_info` for the status.
 */
Ext.namespace('Deluge.preferences');

/**
 * @class Deluge.preferences.Archive
 * @extends Ext.form.FormPanel
 */
Deluge.preferences.Archive = Ext.extend(Ext.form.FormPanel, {
    border: false,
    title: _('Archive'),
    header: false,
    layout: 'form',
    autoScroll: true,

    initComponent: function () {
        Deluge.preferences.Archive.superclass.initComponent.call(this);

        var fieldset = this.add({
            xtype: 'fieldset',
            border: false,
            title: _('Targets'),
            autoHeight: true,
            style: 'margin-bottom: 0px;',
        });
        fieldset.add({
            xtype: 'label',
            text: _(
                'One per line: name | mount path | rclone rc URL (optional, http://user:pass@host:5572). A label picks its target in its own settings.'
            ),
            style: 'display: block; margin-bottom: 6px; opacity: 0.72;',
        });
        this.targets = fieldset.add({
            xtype: 'textarea',
            hideLabel: true,
            anchor: '98%',
            height: 70,
            emptyText: 's4 | /mnt/s4 | http://redeluge:secret@rclone-s4:5572',
        });

        // For every target together: how much the archive does at once.
        fieldset = this.add({
            xtype: 'fieldset',
            border: false,
            title: _('Limits'),
            autoHeight: true,
            labelWidth: 220,
        });
        this.maxMoves = fieldset.add({
            xtype: 'spinnerfield',
            fieldLabel: _('Torrents copied at once:'),
            labelSeparator: '',
            width: 60,
            value: 1,
            minValue: 1,
            maxValue: 16,
            decimalPrecision: 0,
        });
        this.maxUploads = fieldset.add({
            xtype: 'spinnerfield',
            fieldLabel: _('On their way to each target at once:'),
            labelSeparator: '',
            width: 60,
            value: 0,
            minValue: 0,
            maxValue: 64,
            decimalPrecision: 0,
        });
        fieldset.add({
            xtype: 'label',
            text: _(
                'Copied: torrents moving into or out of an archive, all targets together. Each copy goes through the local disk. On their way: per target, torrents being copied in or still being uploaded by rclone (needs its rc URL); 0 for no limit. rclone\'s own --transfers still caps how many files it sends at once.'
            ),
            style: 'display: block; margin: 2px 0 6px 0; opacity: 0.72;',
        });

        fieldset = this.add({
            xtype: 'fieldset',
            border: false,
            title: _('Status'),
            autoHeight: true,
        });
        this.status = fieldset.add({
            xtype: 'panel',
            border: false,
            autoHeight: true,
            html: '',
        });
        fieldset.add({
            xtype: 'button',
            text: _('Refresh'),
            handler: this.loadInfo,
            scope: this,
        });

        this.on('show', this.onPageShow, this);
    },

    onPageShow: function () {
        if (!this.loaded) {
            deluge.client.core.get_config_value('archive', {
                success: function (settings) {
                    var targets = (settings && settings['targets']) || [];
                    this.targets.setValue(
                        targets
                            .map(function (t) {
                                var parts = [t.name, t.path];
                                if (t.rc_url) parts.push(t.rc_url);
                                return parts.join(' | ');
                            })
                            .join('\n')
                    );
                    this.maxMoves.setValue(Deluge.number(settings && settings['max_moves'], 1));
                    this.maxUploads.setValue(
                        Deluge.number(settings && settings['max_uploads'], 0)
                    );
                    this.loaded = true;
                },
                scope: this,
            });
        }
        this.loadInfo();
    },

    loadInfo: function () {
        deluge.client.redeluge.get_archive_info({
            success: function (rows) {
                this.status.update(this.renderInfo(rows || []));
            },
            failure: function () {
                this.status.update(
                    '<i>' + _('The daemon did not answer.') + '</i>'
                );
            },
            scope: this,
        });
    },

    /**
     * One block per target. A grid would need a dozen columns, most of them
     * empty for a target without rclone; a short table per target reads.
     */
    renderInfo: function (rows) {
        var esc = Ext.util.Format.htmlEncode;
        if (!rows.length) {
            return (
                '<i>' +
                _('No targets yet. Add one above and press Apply.') +
                '</i>'
            );
        }
        var orange = 'color: #f0883e; font-weight: bold;';
        return rows
            .map(function (row) {
                var rclone = row.rclone || {};
                var lines = [];
                var add = function (key, value) {
                    if (value === undefined || value === null || value === '')
                        return;
                    lines.push(
                        '<tr><td style="opacity: 0.72; padding-right: 12px; white-space: nowrap;">' +
                            key +
                            '</td><td>' +
                            value +
                            '</td></tr>'
                    );
                };
                add(
                    _('Mount'),
                    row.online
                        ? _('Online')
                        : '<span style="' + orange + '">' + _('Offline') + '</span>'
                );
                add(_('Path'), esc(row.path));
                add(
                    _('rclone'),
                    row.rc === 'ok'
                        ? esc(rclone.version || '')
                        : row.rc === 'none'
                          ? _('no rc URL: status only')
                          : '<span style="' +
                            orange +
                            '">' +
                            _('rc not answering') +
                            '</span>'
                );
                add(_('rc URL'), esc(row.rc_url));
                if (rclone.remote) {
                    add(
                        _('Remote'),
                        esc(rclone.remote + ':' + (rclone.bucket || ''))
                    );
                }
                add(
                    _('Type'),
                    esc(
                        [rclone.type, rclone.provider]
                            .filter(function (v) {
                                return v;
                            })
                            .join(' / ')
                    )
                );
                add(_('Endpoint'), esc(rclone.endpoint));
                add(_('Region'), esc(rclone.region));
                if (rclone.cache_mode) {
                    add(
                        _('Cache'),
                        esc(rclone.cache_mode) +
                            (rclone.cache_max_size > 0
                                ? ', ' +
                                  _('max') +
                                  ' ' +
                                  fsize(rclone.cache_max_size)
                                : '') +
                            (rclone.write_back !== undefined
                                ? ', ' +
                                  _('upload after') +
                                  ' ' +
                                  rclone.write_back +
                                  's'
                                : '')
                    );
                }
                add(
                    _('Labels'),
                    row.labels && row.labels.length
                        ? esc(row.labels.join(', '))
                        : '<i>' + _('none use it') + '</i>'
                );
                return (
                    '<div style="margin: 0 0 10px 0;"><b>' +
                    esc(row.name) +
                    '</b><table style="margin: 2px 0 0 8px;">' +
                    lines.join('') +
                    '</table></div>'
                );
            })
            .join('');
    },

    onApply: function () {
        if (!this.loaded) return;
        var targets = [];
        Ext.each((this.targets.getValue() || '').split('\n'), function (line) {
            var parts = line.split('|').map(function (part) {
                return part.trim();
            });
            if (parts.length < 2 || !parts[0] || !parts[1]) return;
            targets.push({ name: parts[0], path: parts[1], rc_url: parts[2] || '' });
        });
        deluge.client.core.set_config(
            {
                archive: {
                    targets: targets,
                    max_moves: Math.max(1, Math.round(Deluge.number(this.maxMoves.getValue(), 1))),
                    max_uploads: Math.max(0, Math.round(Deluge.number(this.maxUploads.getValue(), 0))),
                },
            },
            { success: this.loadInfo, scope: this }
        );
    },
});
