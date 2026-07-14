# frozen_string_literal: true

require 'json'
require 'spec_helper'
require 'stud_finder/gate'

RSpec.describe StudFinder::Gate do
  def payload(rows, meta: { 'schema_version' => 1 })
    JSON.generate('meta' => meta, 'warnings' => [], 'ruby' => rows, 'javascript' => [])
  end

  def row(path:, score:, evidence:, classification: 'leaf', new_file: false, escalation: '')
    {
      'path' => path,
      'language' => 'ruby',
      'score' => score,
      'evidence' => evidence,
      'class' => classification,
      'new_file' => new_file,
      'escalation' => escalation
    }
  end

  it 'finds trunk-touched rows' do
    result = described_class.call(payload([
                                            row(path: 'app/models/user.rb', score: 0.8, evidence: 1.0,
                                                classification: 'trunk'),
                                            row(path: 'app/models/post.rb', score: 0.4, evidence: 1.0)
                                          ]))

    expect(result.checks.fetch('trunk_touched').map(&:path)).to eq(['app/models/user.rb'])
  end

  describe 'low-evidence high-score thresholds' do
    it 'honors meta.branch_threshold as the high score threshold' do
      result = described_class.call(payload([
                                              row(path: 'below.rb', score: 0.69, evidence: 0.2),
                                              row(path: 'at.rb', score: 0.7, evidence: 0.2)
                                            ], meta: { 'schema_version' => 1, 'branch_threshold' => 0.7 }))

      findings = result.checks.fetch('low_evidence_high_score')
      expect(findings.map(&:path)).to eq(['at.rb'])
      expect(findings.first.reason).to include('Score is >= 0.70')
    end

    it 'defaults the high score threshold to 0.50 when meta is missing' do
      result = described_class.call(payload([row(path: 'default.rb', score: 0.5, evidence: 0.2)], meta: {}))

      findings = result.checks.fetch('low_evidence_high_score')
      expect(findings.map(&:path)).to eq(['default.rb'])
      expect(findings.first.reason).to include('Score is >= 0.50')
    end

    it 'does not treat evidence exactly or above 0.70 as low' do
      result = described_class.call(payload([
                                              row(path: 'exact.rb', score: 0.9, evidence: 0.7),
                                              row(path: 'above.rb', score: 0.9, evidence: 0.71)
                                            ]))

      expect(result.checks.fetch('low_evidence_high_score')).to be_empty
    end

    it 'treats evidence below 0.70 or nil as low when score meets threshold' do
      result = described_class.call(payload([
                                              row(path: 'low.rb', score: 0.5, evidence: 0.69),
                                              row(path: 'nil.rb', score: 0.5, evidence: nil),
                                              row(path: 'score-too-low.rb', score: 0.49, evidence: nil)
                                            ]))

      findings = result.checks.fetch('low_evidence_high_score')
      expect(findings.map(&:path)).to contain_exactly('low.rb', 'nil.rb')
      expect(findings.first.reason).to include('evidence is nil or < 0.70')
    end
  end

  it 'finds new trunk-adjacent rows' do
    result = described_class.call(payload([
                                            row(path: 'new.rb', score: 0.3, evidence: 0.2, new_file: true,
                                                escalation: 'trunk_adjacent'),
                                            row(path: 'old.rb', score: 0.3, evidence: 0.2, new_file: false,
                                                escalation: 'trunk_adjacent')
                                          ]))

    expect(result.checks.fetch('newness_trunk_adjacent').map(&:path)).to eq(['new.rb'])
  end

  it 'renders collapsible markdown for every check' do
    result = described_class.call(payload([row(path: 'app/models/user.rb', score: 0.9, evidence: nil,
                                               classification: 'trunk')]))

    markdown = described_class.markdown(result)

    expect(markdown).to include('## Stud Finder gate')
    expect(markdown).to include('**Mode:** observation')
    expect(markdown).to include('<details open>')
    expect(markdown).to include('<summary><strong>trunk_touched</strong>')
    expect(markdown).to include('<summary><strong>low_evidence_high_score</strong>')
    expect(markdown).to include('<summary><strong>newness_trunk_adjacent</strong>')
    expect(markdown).to include('`app/models/user.rb`')
  end

  it 'raises a gate error on invalid JSON' do
    expect { described_class.call('{') }.to raise_error(StudFinder::Gate::Error, /invalid JSON/)
  end
end
